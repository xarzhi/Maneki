//! 阻尼弹簧：一个值从当前位置弹到目标 —— 起步快、收尾轻微过冲再弹回来。
//!
//! 为什么要这个而不是「按时间 + 缓动函数」插值（现在 `approach` 那种指数逼近）：
//! 缓动的尾巴一定是**匀匀地贴上去**，看不出「弹」；歌词换句要的是 AMLL
//! （applemusic-like-lyrics）/ 苹果音乐那种手感 —— 整列滑上去时**冲过头几个像素、
//! 再弹回来落定**。这只能靠解一个阻尼振子：
//!
//! ```text
//! m·a = -k·x - c·v      x = 离目标多远，v = 速度
//! ```
//!
//! 参数照 AMLL 那套三个量：`mass` / `stiffness`（刚度）/ `damping`（阻力）。
//! 决定「弹不弹」的是**阻尼比** `ζ = c / (2·√(k·m))`：
//!
//! - `ζ = 1` 临界阻尼，刚好不过冲（干净、但没味道）；
//! - `ζ < 1` 欠阻尼，过冲再回弹 —— `ζ` 越小弹得越夸张；
//! - 歌词取 340 / 19 / 1 → `ζ ≈ 0.52`，过冲约 **15%**（一行 26px，冲过头 4px）。
//!
//! 积分用**半隐式欧拉**（先更新速度、再用新速度更新位置），并且把一帧的 `dt`
//! 拆成 ≤ 1/240 秒的子步 —— 这样掉帧到 30fps 也不会把弹簧积炸，而且轨迹和帧率
//! 无关（60fps 走一趟和 144fps 走一趟落在同一个地方）。AMLL 里那套 `Spring`
//! 也是这么做的（它是 1/120 的子步）。

/// 子步上限（秒）。一帧 16ms 会被拆成 4 步走。
const MAX_STEP: f32 = 1.0 / 240.0;
/// 离目标多近算「到了」（单位 = 值本身，歌词里是「行」：0.005 行 ≈ 0.13px）
const SETTLE_DISTANCE: f32 = 0.005;
/// 速度多慢算「停住」（单位 = 值/秒）
const SETTLE_SPEED: f32 = 0.05;

/// 歌词滚动的刚度（越大回弹越快）。
///
/// AMLL 缺省是 400；这里取 340 —— 自然频率 √340 ≈ 18.4 rad/s，一趟约 0.35s 落定，
/// 比 400 稍微从容一点，中文歌词一行字多，滑太快会糊。
const LYRIC_STIFFNESS: f32 = 340.0;
/// 歌词滚动的阻力。和 340 配起来 `ζ ≈ 0.52`：过冲 15%，一眼能看出「弹」。
const LYRIC_DAMPING: f32 = 19.0;

/// 一条弹簧。位置 / 速度 / 目标三个量，外加质量、刚度、阻力。
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    position: f32,
    velocity: f32,
    target: f32,
    mass: f32,
    stiffness: f32,
    damping: f32,
}

impl Spring {
    /// 起一条停在 `position` 的弹簧（还没目标，等于不动）。
    pub fn new(position: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            position,
            velocity: 0.0,
            target: position,
            mass: 1.0,
            stiffness: stiffness.max(1.0),
            damping: damping.max(0.0),
        }
    }

    /// 歌词滚动那条：AMLL 手感（欠阻尼，收尾过冲回弹）。
    pub fn bouncy(position: f32) -> Self {
        Self::new(position, LYRIC_STIFFNESS, LYRIC_DAMPING)
    }

    /// 现在在哪。
    pub fn value(&self) -> f32 {
        self.position
    }

    /// 换目标，**速度保留**。
    ///
    /// 这一点很关键：一句还没落定下一句就来了（快歌、连续两句间隔不到半秒），
    /// 保留速度 = 整列带着动量接着往上走，看着是「一路滑过去」；要是每次换句都
    /// 把速度清零（`kick` 那种「瞬移到起点再松手」），连续换句就会一顿一顿的。
    /// AMLL 换目标时也不动速度。
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// 瞬移过去并把速度清零：换歌 / 拖进度条时用，别让新歌的歌词从上一首的位置滑过来。
    pub fn snap(&mut self, value: f32) {
        self.position = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// 推进 `dt` 秒，返回还有没有在动（false = 已经落定，外面可以省一帧重绘）。
    pub fn tick(&mut self, dt: f32) -> bool {
        // 夹一下：窗口被挂起 / 隔了很久才回来时，大步长会把弹簧积分炸掉
        let mut left = dt.clamp(0.0, 0.1);
        while left > 0.0 {
            let step = left.min(MAX_STEP);
            // 半隐式欧拉：先算受力更新速度，再用**新**速度更新位置
            let force =
                -self.stiffness * (self.position - self.target) - self.damping * self.velocity;
            self.velocity += force / self.mass * step;
            self.position += self.velocity * step;
            left -= step;
        }

        if (self.position - self.target).abs() < SETTLE_DISTANCE
            && self.velocity.abs() < SETTLE_SPEED
        {
            self.snap(self.target);
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 欠阻尼：一定要**冲过目标**再回来，这就是「弹」的全部意义。
    #[test]
    fn bouncy_spring_overshoots_and_then_settles() {
        let mut spring = Spring::bouncy(0.0);
        spring.set_target(1.0);

        let mut peak: f32 = 0.0;
        let mut frames = 0;
        loop {
            let moving = spring.tick(1.0 / 60.0);
            peak = peak.max(spring.value());
            frames += 1;
            if !moving {
                break;
            }
            assert!(frames < 200, "一秒多还没落定，是不是根本不收敛");
        }

        assert!(
            peak > 1.05,
            "没冲过头，看着就是缓动不是弹簧：峰值只有 {peak}"
        );
        assert!(
            peak < 1.35,
            "冲得太狠了（{}），整列会像被甩出去",
            peak
        );
        assert_eq!(spring.value(), 1.0, "最后要精确落在目标上");
        assert!(
            (15..=60).contains(&frames),
            "落定用了 {frames} 帧，太急或者太拖"
        );
    }

    /// 同一条弹簧，60fps 走一趟和 144fps 走一趟得落在同一个地方 ——
    /// 否则在 60Hz 屏上调好的手感，到 144Hz 屏上就变味了（子步的意义）。
    #[test]
    fn trajectory_does_not_depend_on_frame_rate() {
        let mut slow = Spring::bouncy(0.0);
        let mut fast = Spring::bouncy(0.0);
        slow.set_target(4.0);
        fast.set_target(4.0);

        for _ in 0..12 {
            slow.tick(1.0 / 60.0);
        }
        for _ in 0..36 {
            fast.tick(1.0 / 180.0);
        }

        assert!(
            (slow.value() - fast.value()).abs() < 0.02,
            "两种帧率走岔了：{} vs {}",
            slow.value(),
            fast.value()
        );
    }

    /// 换目标时要保留速度：一句没落定又来一句，整列应该接着往上冲，
    /// 而不是在原地重新起速（那会一顿一顿）。
    #[test]
    fn retargeting_keeps_the_velocity() {
        let mut spring = Spring::bouncy(0.0);
        spring.set_target(1.0);
        for _ in 0..6 {
            spring.tick(1.0 / 60.0);
        }
        let moving = spring.velocity;
        assert!(moving > 0.5, "前 6 帧应该已经跑起来了：{moving}");

        spring.set_target(2.0);
        assert_eq!(spring.velocity, moving, "换目标不该清速度");
    }

    /// 瞬移要干净：位置、目标都到，速度归零。
    #[test]
    fn snap_lands_without_momentum() {
        let mut spring = Spring::bouncy(0.0);
        spring.set_target(9.0);
        for _ in 0..5 {
            spring.tick(1.0 / 60.0);
        }
        spring.snap(9.0);
        assert_eq!(spring.value(), 9.0);
        assert!(
            !spring.tick(1.0 / 60.0),
            "瞬移之后应该立刻判成落定，别再往外发重绘"
        );
    }
}
