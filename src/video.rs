//! 视频上屏：把一帧 RGBA 直接作为 GPU 纹理采样绘制。
//!
//! 不能用 `widget::image`——它走图集 + 后台线程异步上传，
//! 1080p 每帧 8MB 的节奏根本追不上，会被整帧跳过（表现为永远黑屏）。
//! 这里用 `widget::shader` 直接拿 wgpu 资源：一次 write_texture + 一个全屏四边形。

use iced::widget::shader;
use iced::wgpu;
use iced::{Rectangle, Size};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub data: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
}

/// 画面怎么塞进窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FitMode {
    /// 默认：保持原始比例，多出来的地方留黑边
    #[default]
    Fit,
    /// 铺满：保持比例放大到填满窗口，超出部分裁掉
    Cover,
    /// 拉伸：直接拉成窗口大小，比例会变形
    Stretch,
}

impl FitMode {
    pub fn label(self) -> &'static str {
        match self {
            FitMode::Fit => "默认",
            FitMode::Cover => "铺满",
            FitMode::Stretch => "拉伸",
        }
    }

    pub const ALL: [FitMode; 3] = [FitMode::Fit, FitMode::Cover, FitMode::Stretch];
}

/// 画面比例。`Auto` 表示用视频自己的比例。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Aspect {
    #[default]
    Auto,
    R16x9,
    R4x3,
    R5x4,
    R21x9,
}

impl Aspect {
    pub fn label(self) -> &'static str {
        match self {
            Aspect::Auto => "自动",
            Aspect::R16x9 => "16:9",
            Aspect::R4x3 => "4:3",
            Aspect::R5x4 => "5:4",
            Aspect::R21x9 => "21:9",
        }
    }

    /// 对应的宽高比；`Auto` 交给调用方取视频自身的比例
    pub fn ratio(self) -> Option<f32> {
        match self {
            Aspect::Auto => None,
            Aspect::R16x9 => Some(16.0 / 9.0),
            Aspect::R4x3 => Some(4.0 / 3.0),
            Aspect::R5x4 => Some(5.0 / 4.0),
            Aspect::R21x9 => Some(21.0 / 9.0),
        }
    }

    pub const ALL: [Aspect; 5] = [
        Aspect::Auto,
        Aspect::R16x9,
        Aspect::R4x3,
        Aspect::R5x4,
        Aspect::R21x9,
    ];
}

/// 色彩调整。全部在片元着色器里做，不走 mpv 的 equalizer
/// （mpv 的 brightness/contrast 只在 gpu 类 VO 上生效，我们用的是 libmpv 软渲染）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Look {
    /// -100..100
    pub brightness: f32,
    /// -100..100
    pub contrast: f32,
    /// -100..100
    pub saturation: f32,
    /// -100..100，映射到 ±180° 色相旋转
    pub hue: f32,
    /// 0..100
    pub sharpen: f32,
}

impl Look {
    pub fn is_neutral(&self) -> bool {
        self.brightness == 0.0
            && self.contrast == 0.0
            && self.saturation == 0.0
            && self.hue == 0.0
            && self.sharpen == 0.0
    }
}

#[derive(Debug, Clone)]
pub struct VideoProgram {
    pub frame: Option<VideoFrame>,
    pub fit: FitMode,
    pub aspect: Aspect,
    pub look: Look,
}

impl<Message> shader::Program<Message> for VideoProgram {
    type State = ();
    type Primitive = VideoPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: iced::mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        VideoPrimitive {
            frame: self.frame.clone(),
            fit: self.fit,
            aspect: self.aspect,
            look: self.look,
        }
    }
}

#[derive(Debug)]
pub struct VideoPrimitive {
    frame: Option<VideoFrame>,
    fit: FitMode,
    aspect: Aspect,
    look: Look,
}

impl shader::Primitive for VideoPrimitive {
    type Pipeline = VideoPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &shader::Viewport,
    ) {
        let Some(frame) = &self.frame else { return };

        pipeline.upload(device, queue, frame);
        pipeline.write_look(queue, self.look, frame.width, frame.height);

        // 显示用的宽高比：用户指定了就按指定的算，否则用视频自己的
        let view_aspect = if bounds.height > 0.0 {
            bounds.width / bounds.height
        } else {
            16.0 / 9.0
        };
        let video_aspect = self
            .aspect
            .ratio()
            .unwrap_or(frame.width as f32 / frame.height as f32);

        // 顶点坐标（NDC，±1 是窗口边缘）与对应的 uv 四角
        let (sx, sy, u0, v0, u1, v1) = match self.fit {
            FitMode::Fit => {
                // 缩到能装进窗口为止，多出来的留黑边
                let (sx, sy) = if video_aspect > view_aspect {
                    (1.0, view_aspect / video_aspect)
                } else {
                    (video_aspect / view_aspect, 1.0)
                };
                (sx, sy, 0.0, 0.0, 1.0, 1.0)
            }
            FitMode::Cover => {
                // 顶点铺满，改 uv：只采样画面中间那块，等价于裁掉溢出部分
                let (ku, kv) = if video_aspect > view_aspect {
                    (view_aspect / video_aspect, 1.0)
                } else {
                    (1.0, video_aspect / view_aspect)
                };
                let u0 = (1.0 - ku) * 0.5;
                let v0 = (1.0 - kv) * 0.5;
                (1.0, 1.0, u0, v0, 1.0 - u0, 1.0 - v0)
            }
            FitMode::Stretch => (1.0, 1.0, 0.0, 0.0, 1.0, 1.0),
        };

        let verts: [[f32; 4]; 4] = [
            [-sx, sy, u0, v0],
            [sx, sy, u1, v0],
            [-sx, -sy, u0, v1],
            [sx, -sy, u1, v1],
        ];
        pipeline.write_vertices(queue, &verts);
    }

    fn draw(
        &self,
        pipeline: &Self::Pipeline,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) -> bool {
        pipeline.draw(render_pass)
    }
}

const ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32x2,
];

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

// params: 亮度(加法) / 对比度(乘) / 饱和度(混) / 色相(弧度)
// extra:  锐化强度
// texel:  单像素 uv 步长，锐化取邻域用
struct Look {
    params: vec4<f32>,
    extra: vec4<f32>,
    texel: vec4<f32>,
};
@group(0) @binding(2) var<uniform> look: Look;

// 标准色相旋转（YIQ 空间），angle 单位是弧度
fn hue_rotate(c: vec3<f32>, angle: f32) -> vec3<f32> {
    let cs = cos(angle);
    let sn = sin(angle);
    let r = c.r * (0.299 + 0.701 * cs + 0.168 * sn)
          + c.g * (0.587 - 0.587 * cs + 0.330 * sn)
          + c.b * (0.114 - 0.114 * cs - 0.497 * sn);
    let g = c.r * (0.299 - 0.299 * cs - 0.328 * sn)
          + c.g * (0.587 + 0.413 * cs + 0.035 * sn)
          + c.b * (0.114 - 0.114 * cs + 0.292 * sn);
    let b = c.r * (0.299 - 0.300 * cs + 1.250 * sn)
          + c.g * (0.587 - 0.588 * cs - 1.050 * sn)
          + c.b * (0.114 + 0.886 * cs - 0.203 * sn);
    return vec3<f32>(r, g, b);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(tex, samp, in.uv).rgb;

    // 锐化：3x3 十字邻域做非锐化掩模
    let amount = look.extra.x;
    if (amount > 0.001) {
        let t = look.texel.xy;
        let n = textureSample(tex, samp, in.uv + vec2<f32>(t.x, 0.0)).rgb
              + textureSample(tex, samp, in.uv - vec2<f32>(t.x, 0.0)).rgb
              + textureSample(tex, samp, in.uv + vec2<f32>(0.0, t.y)).rgb
              + textureSample(tex, samp, in.uv - vec2<f32>(0.0, t.y)).rgb;
        let blur = (c * 4.0 + n) / 8.0;
        c = c + (c - blur) * amount;
    }

    // 亮度 / 对比度 / 饱和度 / 色相
    //
    // 在**近似显示空间**里算，而不是直接在线性值上算：
    // 纹理是 sRGB 格式，采样出来已经解码成线性，线性空间里 ±0.25 的加法会把
    // 暗部抬得很厉害、亮部几乎不动，手感像坏掉。用 sqrt / 平方近似 gamma2
    // （比 pow(x, 1/2.2) 便宜一个数量级），调出来的观感跟常见播放器一致；
    // 参数全中性时 sqrt→平方正好抵消，画面一个像素都不会变。
    let p = look.params;
    if (any(p != vec4<f32>(0.0, 1.0, 1.0, 0.0))) {
        var g = sqrt(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
        g = g + vec3<f32>(p.x);
        g = (g - vec3<f32>(0.5)) * p.y + vec3<f32>(0.5);
        let luma = dot(g, vec3<f32>(0.2126, 0.7152, 0.0722));
        g = mix(vec3<f32>(luma), g, p.z);
        if (abs(p.w) > 0.0001) {
            g = hue_rotate(g, p.w);
        }
        g = clamp(g, vec3<f32>(0.0), vec3<f32>(1.0));
        c = g * g;
    }

    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

#[derive(Debug)]
pub struct VideoPipeline {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    texture: Option<(u32, u32, wgpu::Texture, wgpu::BindGroup)>,
    vertices: wgpu::Buffer,
    /// 色彩调整参数（3 个 vec4 = 48 字节，见着色器里的 `Look`）
    look_buffer: wgpu::Buffer,
    /// mpv 交给我们的是「已经按 sRGB 编码好的字节」。
    ///
    /// 纹理格式必须和渲染目标的色彩空间对齐，否则会多吃一次转换：
    /// - 目标是 sRGB → 采样时 GPU 会自动 sRGB 解码成线性，写回 sRGB 目标时再编码，
    ///   一解一编正好抵消，所以纹理也声明成 `Rgba8UnormSrgb`
    /// - 目标不是 sRGB → 着色器输出原样落屏，纹理就该是 `Rgba8Unorm`，直接拿原始字节
    ///
    /// 之前无条件写死 `Rgba8UnormSrgb`，遇上非 sRGB 目标时画面会整体偏暗（这正是之前的 bug）。
    tex_format: wgpu::TextureFormat,
    /// 上一次上传的那帧，指针相同就跳过 8MB 的重复上传
    uploaded: usize,
}

impl shader::Pipeline for VideoPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("maneki.video.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("maneki.video.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("maneki.video.layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("maneki.video.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &ATTRS,
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("maneki.video.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("maneki.video.vertices"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let look_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("maneki.video.look"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bgl,
            sampler,
            texture: None,
            vertices,
            look_buffer,
            tex_format: if format.is_srgb() {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            uploaded: 0,
        }
    }
}

impl VideoPipeline {
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &VideoFrame) {
        // 同一帧重复来到 prepare（鼠标移动、滑块动画都会触发重绘）不必再传 8MB
        let ptr = Arc::as_ptr(&frame.data) as usize;
        if ptr == self.uploaded && self.texture.is_some() {
            return;
        }
        self.uploaded = ptr;

        let need_new = match &self.texture {
            Some((w, h, _, _)) => *w != frame.width || *h != frame.height,
            None => true,
        };

        if need_new {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("maneki.video.tex"),
                size: wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.tex_format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("maneki.video.bind"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.look_buffer.as_entire_binding(),
                    },
                ],
            });
            self.texture = Some((frame.width, frame.height, tex, bind_group));
        }

        if let Some((_, _, tex, _)) = &self.texture {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &frame.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(frame.width * 4),
                    rows_per_image: Some(frame.height),
                },
                wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn write_vertices(&self, queue: &wgpu::Queue, verts: &[[f32; 4]; 4]) {
        queue.write_buffer(&self.vertices, 0, as_bytes(verts.as_slice()));
    }

    /// 把 -100..100 的界面数值翻译成着色器能直接用的系数。
    ///
    /// 中性值（0/0/0/0/0）算出来正好是 `(0,1,1,0)`，着色器里那个 `any(...)` 判断
    /// 会整段跳过，所以「没调过色」时和不带这几行代码的版本完全等价 —— 不会因为
    /// 多了一次矩阵运算就出现色偏。
    ///
    /// 亮度用 0.45 而不是 1.0：着色器在 gamma2 空间里做加法，±100 就对应
    /// 显示亮度的 ±45%，再大就不是「调亮度」而是「一刀切黑/白」了。
    fn write_look(&self, queue: &wgpu::Queue, look: Look, w: u32, h: u32) {
        let brightness = look.brightness / 100.0 * 0.45;
        let contrast = 1.0 + look.contrast / 100.0 * 0.6;
        let saturation = 1.0 + look.saturation / 100.0;
        let hue = look.hue / 100.0 * std::f32::consts::PI;
        let sharpen = look.sharpen / 100.0 * 2.0;

        let data: [f32; 12] = [
            brightness,
            contrast,
            saturation,
            hue,
            sharpen,
            0.0,
            0.0,
            0.0,
            1.0 / w.max(1) as f32,
            1.0 / h.max(1) as f32,
            0.0,
            0.0,
        ];
        queue.write_buffer(&self.look_buffer, 0, as_bytes(&data));
    }

    fn draw(&self, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some((_, _, _, bind_group)) = &self.texture else {
            return true;
        };

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertices.slice(..));
        render_pass.draw(0..4, 0..1);
        true
    }
}

/// 把一段 f32 内存原样当字节切片交出去，省掉 bytemuck 依赖。
fn as_bytes<T>(slice: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            slice.as_ptr() as *const u8,
            std::mem::size_of_val(slice),
        )
    }
}

/// 供上层做尺寸预算（避免把 4K 原帧塞进 CPU 拷贝）。
pub fn fit(source: Size<u32>, max: Size<u32>) -> Size<u32> {
    if source.width <= max.width && source.height <= max.height {
        return Size::new(source.width.max(2), source.height.max(2));
    }
    let scale = (max.width as f32 / source.width as f32)
        .min(max.height as f32 / source.height as f32);
    Size::new(
        ((source.width as f32 * scale) as u32).max(2),
        ((source.height as f32 * scale) as u32).max(2),
    )
}
