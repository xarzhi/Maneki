//! 全屏静置时把鼠标指针藏起来的那个「最外层包装」。
//!
//! iced 没有「请把指针藏起来」这样的接口 —— 指针长什么样，完全由**根组件**
//! 报出来的 [`mouse::Interaction`] 决定（`iced_runtime` 的 `UserInterface::update`
//! 里，只有那一处会把交互值交给窗口）。报 `Interaction::Hidden` 时，
//! `iced_winit` 才会调 `set_cursor_visible(false)`。
//!
//! 为什么不自己调 Win32 的 `ShowCursor(FALSE)`：
//!
//! - winit 内部用一个**进程级静态标记**记着「现在藏没藏」，只有标记翻转时才真的
//!   调 `ShowCursor`（见 `winit::platform_impl::windows::util::set_cursor_hidden`）。
//!   我们绕过它自己藏一次，它那个标记还是「没藏」，之后 iced 让指针恢复时它
//!   判定「本来就没藏」→ 不调 `ShowCursor(TRUE)` → **指针再也回不来了**。
//! - 走这条路，隐藏/恢复由 winit 自己配对，进程退出、窗口关掉都不会把桌面搞成
//!   「有个看不见的指针」。
//!
//! 为什么也不是「在所有内容上面盖一层 `mouse_area(..).interaction(Hidden)`」：
//! `stack` 里只要上层报出非 `None` 的交互，下面的兄弟层拿到的 cursor 就会被
//! `levitate()`（"浮空"），悬停和点击全部失效 —— 指针是藏住了，可那时整个界面
//! 也点不动了。包在根上没有这个问题：整棵树还是原来那一棵，只是换成我们替它
//! 报指针样式。

use iced::advanced::layout;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// 透传一切、只在 `hidden` 时把指针样式改成 `Hidden` 的包装组件。
pub struct HideCursor<'a, Message> {
    content: Element<'a, Message>,
    hidden: bool,
}

impl<'a, Message> HideCursor<'a, Message> {
    pub fn new(content: Element<'a, Message>, hidden: bool) -> Self {
        Self { content, hidden }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for HideCursor<'_, Message> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // 一定要透传：滑块、滚动区、文本选择这些都是靠 operate 找到自己的
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    /// 唯一一处跟原树不一样的地方。
    ///
    /// 只认「指针确实在窗口里」：`cursor` 不在（鼠标移出窗口 / 在别的窗口上）时
    /// 不能报 Hidden —— `ShowCursor` 是全局的，那会把**整个桌面**的指针藏掉。
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if self.hidden && cursor.is_over(layout.bounds()) {
            return mouse::Interaction::Hidden;
        }

        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
        // 右键菜单、popover 都是 overlay，必须透传，否则它们会凭空消失
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: 'a> From<HideCursor<'a, Message>> for Element<'a, Message> {
    fn from(widget: HideCursor<'a, Message>) -> Self {
        Element::new(widget)
    }
}
