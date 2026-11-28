use gpui::{Context, IntoElement, Render, TestAppContext, Window, div};

struct Counter(u32);

impl Render for Counter {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[gpui::test]
fn view_state_updates_and_reads_back(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Counter(0));
    let view = window.root(cx).unwrap();
    view.update(cx, |c, _| c.0 += 2);
    assert_eq!(view.read_with(cx, |c, _| c.0), 2);
}
