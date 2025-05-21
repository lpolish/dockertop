use anyhow::{Context, Result};
use bollard::container::{ListContainersOptions, Stats, StatsOptions};
use bollard::Docker;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::TryStreamExt;
use std::{
    io,
    time::{Duration, Instant},
};
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};

struct ContainerStats {
    #[allow(dead_code)]
    id: String,
    name: String,
    cpu_usage: f64,
    memory_usage: u64,
    memory_limit: u64,
    status: String,
    created: String,
}

struct App {
    containers: Vec<ContainerStats>,
    selected_index: usize,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            containers: Vec::new(),
            selected_index: 0,
            should_quit: false,
        }
    }

    async fn update_stats(&mut self, docker: &Docker) -> Result<()> {
        let options = ListContainersOptions::<String> {
            all: true,
            ..Default::default()
        };

        let containers = docker
            .list_containers(Some(options))
            .await
            .context("Failed to list containers")?;

        self.containers = Vec::with_capacity(containers.len());

        for container in containers {
            if let Some(id) = container.id {
                let stats = docker
                    .stats(&id, None::<StatsOptions>)
                    .try_next()
                    .await
                    .context("Failed to get container stats")?
                    .unwrap();

                let cpu_usage = calculate_cpu_usage(&stats);
                let memory_usage = stats.memory_stats.usage.unwrap_or(0);
                let memory_limit = stats.memory_stats.limit.unwrap_or(1);

                self.containers.push(ContainerStats {
                    id,
                    name: container.names.unwrap_or_default()[0].trim_start_matches('/').to_string(),
                    cpu_usage,
                    memory_usage,
                    memory_limit,
                    status: container.status.unwrap_or_default(),
                    created: container.created.map(|t| t.to_string()).unwrap_or_default(),
                });
            }
        }

        Ok(())
    }
}

fn calculate_cpu_usage(stats: &Stats) -> f64 {
    let cpu_delta = stats.cpu_stats.cpu_usage.total_usage - stats.precpu_stats.cpu_usage.total_usage;
    let system_delta = stats.cpu_stats.system_cpu_usage.unwrap_or(0) - stats.precpu_stats.system_cpu_usage.unwrap_or(0);
    
    if system_delta > 0 {
        (cpu_delta as f64 / system_delta as f64) * 100.0
    } else {
        0.0
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}

fn ui<B: Backend>(f: &mut Frame<B>, app: &App) {
    // Create a vertical layout for the entire screen
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),  // Main content
            Constraint::Length(3), // Help bar
        ].as_ref())
        .split(f.size());

    // Split the main content area horizontally
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(chunks[0]);

    // Container list with enhanced styling
    let items: Vec<ListItem> = app
        .containers
        .iter()
        .map(|c| {
            let memory_percent = (c.memory_usage as f64 / c.memory_limit as f64) * 100.0;
            let status_style = match c.status.as_str() {
                "running" => Style::default().fg(Color::Green),
                "exited" => Style::default().fg(Color::Red),
                _ => Style::default().fg(Color::Yellow),
            };
            
            ListItem::new(Spans::from(vec![
                Span::styled(
                    format!("{} [{}] - CPU: {:.1}% | MEM: {:.1}%",
                        c.name, c.status, c.cpu_usage, memory_percent
                    ),
                    status_style
                ),
            ]))
        })
        .collect();

    let containers = List::new(items)
        .block(
            Block::default()
                .title(" Containers (↑/↓ to navigate) ")
                .borders(Borders::ALL)
                .border_type(tui::widgets::BorderType::Rounded)
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = tui::widgets::ListState::default();
    state.select(Some(app.selected_index));
    f.render_stateful_widget(containers, main_chunks[0], &mut state);

    // Container details with enhanced styling
    if let Some(container) = app.containers.get(app.selected_index) {
        let details = vec![
            format!("Container: {}", container.name),
            format!("Status: {}", container.status),
            format!("CPU Usage: {:.1}%", container.cpu_usage),
            format!(
                "Memory Usage: {:.1}% ({})",
                (container.memory_usage as f64 / container.memory_limit as f64) * 100.0,
                format_bytes(container.memory_usage)
            ),
            format!("Created: {}", container.created),
        ];

        let details_text = details.join("\n");
        let details_widget = Paragraph::new(details_text)
            .block(
                Block::default()
                    .title(" Container Details ")
                    .borders(Borders::ALL)
                    .border_type(tui::widgets::BorderType::Rounded)
            );

        f.render_widget(details_widget, main_chunks[1]);
    }

    // Help bar at the bottom
    let help_text = vec![
        Span::styled("q", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw(": Quit  "),
        Span::styled("↑/↓", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw(": Navigate  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw(": Select Container"),
    ];

    let help_widget = Paragraph::new(Spans::from(help_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(tui::widgets::BorderType::Rounded)
        );

    f.render_widget(help_widget, chunks[1]);
}

#[tokio::main]
async fn main() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run it
    let docker = Docker::connect_with_local_defaults()?;
    let mut app = App::new();
    let tick_rate = Duration::from_millis(2000);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui(f, &app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => app.should_quit = true,
                    KeyCode::Up => {
                        if app.selected_index > 0 {
                            app.selected_index -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if app.selected_index < app.containers.len().saturating_sub(1) {
                            app.selected_index += 1;
                        }
                    }
                    _ => {}
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.update_stats(&docker).await?;
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}
