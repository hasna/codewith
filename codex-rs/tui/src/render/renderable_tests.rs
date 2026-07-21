use super::*;
use pretty_assertions::assert_eq;
use std::cell::Cell;

#[test]
fn flex_caches_child_height_across_frame_passes() {
    struct CountingRenderable<'a>(&'a Cell<usize>);

    impl Renderable for CountingRenderable<'_> {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}

        fn desired_height(&self, _width: u16) -> u16 {
            self.0.set(self.0.get() + 1);
            1
        }

        fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
            Some((area.x, area.y))
        }
    }

    let calls = Cell::new(0);
    let renderable = CountingRenderable(&calls);
    let mut flex = FlexRenderable::new();
    flex.push(/*flex*/ 1, RenderableItem::Borrowed(&renderable));
    let area = Rect::new(
        /*x*/ 0, /*y*/ 0, /*width*/ 80, /*height*/ 10,
    );
    let mut buf = Buffer::empty(area);

    assert_eq!(flex.desired_height(area.width), 1);
    flex.render(area, &mut buf);
    assert_eq!(flex.cursor_pos(area), Some((0, 0)));
    assert!(matches!(
        flex.cursor_style(area),
        crossterm::cursor::SetCursorStyle::DefaultUserShape
    ));
    assert_eq!(calls.get(), 1);

    assert_eq!(flex.desired_height(/*width*/ 100), 1);
    assert_eq!(calls.get(), 2);
}
