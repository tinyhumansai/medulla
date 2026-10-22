use super::agents::AgentRow;
use ratatui::{
    layout::Rect,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub(super) fn draw(frame: &mut Frame, area: Rect, row: Option<&AgentRow>) {
    let Some(row) = row else {
        frame.render_widget(
            Paragraph::new("No agent selected").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Agent detail · Esc back "),
            ),
            area,
        );
        return;
    };
    let body = format!("PID       {}\nAgent     {}\nOrigin    {}\nWorkspace {}\nThreads   {}\nMemory    {} bytes\nCPU       {:.1}%\nRuntime   {} seconds\nParent    {}\n\nEsc back · t/k/s signal",row.pid,row.kind,row.origin,row.workspace,row.threads,row.memory,row.cpu,row.runtime,row.ppid.map_or("—".into(),|pid|pid.to_string()));
    frame.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Agent detail · Esc back "),
        ),
        area,
    );
}
