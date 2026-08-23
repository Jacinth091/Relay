use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Paragraph, Wrap},
};

use crate::state::AppState;
use crate::util::activity_home;

pub(crate) fn render_dashboard(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = activity_home(state);
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}
