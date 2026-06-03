use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Paragraph, Row, Sparkline, Table, TableState, Wrap,
};
use ratatui::Frame;
use std::io;
use std::time::{Duration, Instant};
use sysinfo::{RefreshKind, System};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Processes,
    Overview,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Cpu = 0,
    Mem = 1,
    Pid = 2,
    Name = 3,
}

impl SortBy {
    fn label(&self) -> &str {
        match self {
            SortBy::Cpu => "CPU%",
            SortBy::Mem => "MEM%",
            SortBy::Pid => "PID",
            SortBy::Name => "NAME",
        }
    }

    fn variants() -> [SortBy; 4] {
        [SortBy::Cpu, SortBy::Mem, SortBy::Pid, SortBy::Name]
    }
}

struct NetMonitor {
    prev_rx: u64,
    prev_tx: u64,
    speed_rx: f64,
    speed_tx: f64,
}

impl NetMonitor {
    fn new() -> Self {
        Self {
            prev_rx: 0,
            prev_tx: 0,
            speed_rx: 0.0,
            speed_tx: 0.0,
        }
    }

    fn update(&mut self, rx: u64, tx: u64, elapsed: f64) {
        if elapsed > 0.0 {
            let rx_diff = rx.saturating_sub(self.prev_rx);
            let tx_diff = tx.saturating_sub(self.prev_tx);
            self.speed_rx = rx_diff as f64 / elapsed;
            self.speed_tx = tx_diff as f64 / elapsed;
        }
        self.prev_rx = rx;
        self.prev_tx = tx;
    }
}

struct App {
    system: System,
    processes: Vec<(u32, String, f32, f64, String)>,
    filter: String,
    search_focused: bool,
    sort_by: SortBy,
    sort_desc: bool,
    should_quit: bool,
    show_help: bool,
    total_mem: u64,
    used_mem: u64,
    cpu_usage: f32,
    net: NetMonitor,
    total_rx: u64,
    total_tx: u64,
    gpu_usage: f32,
    gpu_mem_used: u64,
    gpu_mem_total: u64,
    gpu_last_check: Instant,
    table_state: TableState,
    cpu_history: Vec<u64>,
    mem_history: Vec<u64>,
    page: Page,
    hostname: String,
    os_name: String,
    kernel: String,
    cpu_model: String,
    num_cpus: usize,
    total_swap: u64,
    used_swap: u64,
    load_one: f64,
    load_five: f64,
    load_fifteen: f64,
}

impl App {
    fn new() -> Self {
        let system = System::new_with_specifics(
            RefreshKind::everything(),
        );

        let total_mem = system.total_memory();
        let used_mem = system.used_memory();
        let cpu_model = system.cpus().first().map(|c| c.brand().to_string()).unwrap_or_default();
        let num_cpus = system.cpus().len();
        let total_swap = system.total_swap();
        let used_swap = system.used_swap();

        let mut app = Self {
            system,
            processes: Vec::new(),
            filter: String::new(),
            search_focused: false,
            sort_by: SortBy::Cpu,
            sort_desc: true,
            should_quit: false,
            show_help: false,
            total_mem,
            used_mem,
            cpu_usage: 0.0,
            net: NetMonitor::new(),
            total_rx: 0,
            total_tx: 0,
            gpu_usage: 0.0,
            gpu_mem_used: 0,
            gpu_mem_total: 0,
            gpu_last_check: Instant::now(),
            table_state: TableState::default().with_selected(0),
            cpu_history: Vec::with_capacity(60),
            mem_history: Vec::with_capacity(60),
            page: Page::Processes,
            hostname: System::host_name().unwrap_or_default(),
            os_name: System::name().unwrap_or_default(),
            kernel: System::kernel_version().unwrap_or_default(),
            cpu_model,
            num_cpus,
            total_swap,
            used_swap,
            load_one: 0.0,
            load_five: 0.0,
            load_fifteen: 0.0,
        };
        app.refresh();
        app
    }

    fn next(&mut self) {
        let len = self.processes.len();
        if len == 0 {
            return;
        }
        let i = self.table_state.selected().unwrap_or(0);
        if i + 1 < len {
            self.table_state.select(Some(i + 1));
        }
    }

    fn prev(&mut self) {
        let i = self.table_state.selected().unwrap_or(0);
        if i > 0 {
            self.table_state.select(Some(i - 1));
        }
    }

    fn page_up(&mut self) {
        let i = self.table_state.selected().unwrap_or(0);
        self.table_state.select(Some(i.saturating_sub(10)));
    }

    fn page_down(&mut self) {
        let len = self.processes.len();
        if len == 0 {
            return;
        }
        let i = self.table_state.selected().unwrap_or(0);
        self.table_state.select(Some((i + 10).min(len - 1)));
    }

    fn toggle_sort(&mut self) {
        let variants = SortBy::variants();
        let idx = variants.iter().position(|s| *s == self.sort_by).unwrap_or(0);
        self.sort_by = variants[(idx + 1) % variants.len()];
        self.sort();
    }

    fn sort(&mut self) {
        let desc = self.sort_desc;
        match self.sort_by {
            SortBy::Cpu => {
                if desc {
                    self.processes.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
                } else {
                    self.processes.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
                }
            }
            SortBy::Mem => {
                if desc {
                    self.processes.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
                } else {
                    self.processes.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
                }
            }
            SortBy::Pid => {
                if desc {
                    self.processes.sort_by(|a, b| b.0.cmp(&a.0));
                } else {
                    self.processes.sort_by(|a, b| a.0.cmp(&b.0));
                }
            }
            SortBy::Name => {
                if desc {
                    self.processes.sort_by(|a, b| b.4.cmp(&a.4));
                } else {
                    self.processes.sort_by(|a, b| a.4.cmp(&b.4));
                }
            }
        }
    }

    fn refresh(&mut self) {
        self.system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        self.system.refresh_memory();
        self.system.refresh_cpu_usage();

        self.cpu_usage = self.system.global_cpu_usage();
        self.total_mem = self.system.total_memory();
        self.used_mem = self.system.used_memory();

        let mem_pct = if self.total_mem > 0 {
            (self.used_mem as f64 / self.total_mem as f64 * 100.0) as u64
        } else {
            0
        };
        self.cpu_history.push(self.cpu_usage as u64);
        self.mem_history.push(mem_pct);
        if self.cpu_history.len() > 60 {
            self.cpu_history.remove(0);
        }
        if self.mem_history.len() > 60 {
            self.mem_history.remove(0);
        }

        let now = Instant::now();
        let gpu_elapsed = now.duration_since(self.gpu_last_check).as_secs_f64();
        if gpu_elapsed >= 2.0 {
            self.refresh_gpu();
            self.gpu_last_check = now;
        }

        let mut processes: Vec<(u32, String, f32, f64, String)> = Vec::new();
        for (pid, process) in self.system.processes() {
            let name = process.name().to_string_lossy().to_string();
            if !self.filter.is_empty() && !name.to_lowercase().contains(&self.filter.to_lowercase()) {
                continue;
            }
            let cpu = process.cpu_usage();
            let mem = process.memory() as f64 / 1024.0 / 1024.0;
            let state = match process.status() {
                sysinfo::ProcessStatus::Run => 'R',
                sysinfo::ProcessStatus::Sleep => 'S',
                sysinfo::ProcessStatus::Zombie => 'Z',
                sysinfo::ProcessStatus::Stop => 'T',
                sysinfo::ProcessStatus::Idle => 'I',
                _ => '?',
            };
            processes.push((pid.as_u32(), state.to_string(), cpu, mem, name));
        }

        self.processes = processes;
        self.sort();

        self.total_swap = self.system.total_swap();
        self.used_swap = self.system.used_swap();
        let load = System::load_average();
        self.load_one = load.one;
        self.load_five = load.five;
        self.load_fifteen = load.fifteen;

        if let Ok(net_bytes) = read_net_bytes() {
            let now = Instant::now();
            let elapsed = now.duration_since(self.gpu_last_check).as_secs_f64();
            self.net.update(net_bytes.0, net_bytes.1, elapsed);
            self.total_rx = net_bytes.0;
            self.total_tx = net_bytes.1;
        }

        let selected = self.table_state.selected().unwrap_or(0);
        if !self.processes.is_empty() && selected >= self.processes.len() {
            self.table_state.select(Some(self.processes.len().saturating_sub(1)));
        }
    }

    fn refresh_gpu(&mut self) {
        let nvidia = get_gpu_usage_nvidia();
        if let Some((usage, mem_used, mem_total)) = nvidia {
            self.gpu_usage = usage;
            self.gpu_mem_used = mem_used;
            self.gpu_mem_total = mem_total;
        } else {
            let intel = read_intel_gpu_usage();
            if let Some(usage) = intel {
                self.gpu_usage = usage;
            }
        }
    }

    fn kill_selected(&mut self) {
        if let Some(selected) = self.table_state.selected() {
            if selected < self.processes.len() {
                let pid = self.processes[selected].0;
                let _ = std::process::Command::new("kill")
                    .arg(pid.to_string())
                    .spawn();
            }
        }
    }
}

fn read_net_bytes() -> io::Result<(u64, u64)> {
    let content = std::fs::read_to_string("/proc/net/dev")?;
    let mut total_rx = 0u64;
    let mut total_tx = 0u64;
    for line in content.lines().skip(2) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 10 {
            if let Ok(rx) = parts[1].parse::<u64>() {
                total_rx += rx;
            }
            if let Ok(tx) = parts[9].parse::<u64>() {
                total_tx += tx;
            }
        }
    }
    Ok((total_rx, total_tx))
}

fn get_gpu_usage_nvidia() -> Option<(f32, u64, u64)> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    let s = String::from_utf8(output.stdout).ok()?;
    let parts: Vec<&str> = s.trim().split(',').map(|s| s.trim()).collect();
    if parts.len() >= 3 {
        let usage = parts[0].parse::<f32>().ok()?;
        let mem_used = parts[1].parse::<u64>().ok()?;
        let mem_total = parts[2].parse::<u64>().ok()?;
        Some((usage, mem_used, mem_total))
    } else {
        None
    }
}

fn read_intel_gpu_usage() -> Option<f32> {
    let vendor = std::fs::read_to_string("/sys/class/drm/card0/device/vendor").ok()?;
    if vendor.trim() != "0x8086" {
        return None;
    }
    let cur = std::fs::read_to_string("/sys/class/drm/card0/gt/gt0/rps_cur_freq").ok()?;
    let max = std::fs::read_to_string("/sys/class/drm/card0/gt/gt0/rps_max_freq").ok()?;
    let cur_freq: f32 = cur.trim().parse().ok()?;
    let max_freq: f32 = max.trim().parse().ok()?;
    if max_freq > 0.0 {
        Some((cur_freq / max_freq) * 100.0)
    } else {
        None
    }
}

fn format_speed(bytes_per_sec: f64) -> String {
    let bps = bytes_per_sec;
    if bps >= 1_000_000_000.0 {
        format!("{:.1} GB/s", bps / 1_000_000_000.0)
    } else if bps >= 1_000_000.0 {
        format!("{:.1} MB/s", bps / 1_000_000.0)
    } else if bps >= 1_000.0 {
        format!("{:.1} KB/s", bps / 1_000.0)
    } else {
        format!("{:.0} B/s", bps)
    }
}

#[allow(dead_code)]
fn format_mem_gb(kb: u64) -> String {
    let gb = kb as f64 / 1024.0 / 1024.0;
    format!("{:.1} GiB", gb)
}

fn header_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn style_accent() -> Style {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
}

fn style_dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn pid_color(pid: u32) -> Color {
    let hue = (pid as f64 * 0.618033988749895) % 1.0;
    let r = (hue * 6.0) as u8;
    match r {
        0 => Color::Rgb(255, (hue * 6.0 * 255.0) as u8, 0),
        1 => Color::Rgb((255.0 - (hue * 6.0 - 1.0) * 255.0) as u8, 255, 0),
        2 => Color::Rgb(0, 255, ((hue * 6.0 - 2.0) * 255.0) as u8),
        3 => Color::Rgb(0, (255.0 - (hue * 6.0 - 3.0) * 255.0) as u8, 255),
        4 => Color::Rgb(((hue * 6.0 - 4.0) * 255.0) as u8, 0, 255),
        _ => Color::Rgb(255, 0, (255.0 - (hue * 6.0 - 5.0) * 255.0) as u8),
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((r.height * (100 - percent_y)) / 200),
            Constraint::Length((r.height * percent_y) / 100),
            Constraint::Length((r.height * (100 - percent_y)) / 200),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((r.width * (100 - percent_x)) / 200),
            Constraint::Length((r.width * percent_x) / 100),
            Constraint::Length((r.width * (100 - percent_x)) / 200),
        ])
        .split(popup_layout[1])[1]
}

fn draw_top_bar(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24),
            Constraint::Length(24),
            Constraint::Length(24),
            Constraint::Min(10),
            Constraint::Length(20),
        ])
        .split(area);

    let cpu_style = Style::default().fg(Color::Cyan);
    let cpu_label = format!(" CPU {:5.1}% ", app.cpu_usage);
    let cpu_block = Block::default()
        .title(cpu_label)
        .borders(Borders::ALL)
        .border_type(BorderType::Plain);
    let cpu_spark = Sparkline::default()
        .block(cpu_block)
        .style(cpu_style)
        .data(&app.cpu_history)
        .max(100)
        .bar_set(symbols::bar::NINE_LEVELS);
    frame.render_widget(cpu_spark, chunks[0]);

    let mem_pct = if app.total_mem > 0 {
        (app.used_mem as f64 / app.total_mem as f64 * 100.0) as u64
    } else {
        0
    };
    let mem_style = Style::default().fg(Color::Yellow);
    let mem_label = format!(" MEM {:5.1}% ", mem_pct as f32);
    let mem_block = Block::default()
        .title(mem_label)
        .borders(Borders::ALL)
        .border_type(BorderType::Plain);
    let mem_spark = Sparkline::default()
        .block(mem_block)
        .style(mem_style)
        .data(&app.mem_history)
        .max(100)
        .bar_set(symbols::bar::NINE_LEVELS);
    frame.render_widget(mem_spark, chunks[1]);

    if app.gpu_usage > 0.0 {
        let gpu_label = format!(" GPU {:5.1}% ", app.gpu_usage);
        let gpu_block = Block::default()
            .title(gpu_label)
            .borders(Borders::ALL)
            .border_type(BorderType::Plain);
        let gpu_spark = Sparkline::default()
            .block(gpu_block)
            .style(Style::default().fg(Color::Green))
            .data(&[app.gpu_usage as u64])
            .max(100)
            .bar_set(symbols::bar::NINE_LEVELS);
        frame.render_widget(gpu_spark, chunks[2]);
    }

    let net_text = format!(
        "↓{} ↑{}",
        format_speed(app.net.speed_rx),
        format_speed(app.net.speed_tx),
    );
    let net_para = Paragraph::new(Line::from(Span::styled(net_text, Style::default().fg(Color::DarkGray))))
        .alignment(Alignment::Right)
        .block(Block::default().borders(Borders::NONE));
    frame.render_widget(net_para, chunks[4]);
}

fn draw_table(frame: &mut Frame, area: Rect, app: &mut App) {
    let header_cells = ["PID", "S", "CPU%", "MEM%", "NAME"]
        .iter()
        .map(|c| Cell::from(Line::from(Span::styled(*c, header_style()))));
    let header = Row::new(header_cells)
        .height(1)
        .style(header_style());

    let rows: Vec<Row> = app
        .processes
        .iter()
        .enumerate()
        .map(|(idx, (pid, state, cpu, mem, name))| {
            let selected = app.table_state.selected() == Some(idx);
            let row_style = if selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };
            let pid_style = Style::default().fg(pid_color(*pid));
            Row::new(vec![
                Cell::from(Line::from(Span::styled(pid.to_string(), pid_style))),
                Cell::from(Line::from(Span::styled(state.clone(), style_dim()))),
                Cell::from(Line::from(Span::styled(
                    format!("{:5.1}", cpu),
                    Style::default().fg(if *cpu > 50.0 { Color::Red } else { Color::Green }),
                ))),
                Cell::from(Line::from(Span::styled(
                    format!("{:5.1}", mem),
                    Style::default().fg(Color::Yellow),
                ))),
                Cell::from(Line::from(Span::styled(name.clone(), Style::default()))),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Length(3),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Min(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Processes "));

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn draw_search_bar(frame: &mut Frame, area: Rect, app: &App) {
    let text = if app.search_focused {
        format!("/{}_", app.filter)
    } else {
        "/ search...".to_string()
    };
    let para = Paragraph::new(text)
        .style(if app.search_focused {
            Style::default().fg(Color::Cyan)
        } else {
            style_dim()
        })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Search")
                .border_type(BorderType::Plain),
        );
    frame.render_widget(para, area);
}

fn draw_help(frame: &mut Frame, area: Rect, app: &App) {
    let help_text = vec![
        Line::from(vec![
            Span::styled("Help", Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("q / Esc  ", style_accent()),
            Span::from("Quit"),
        ]),
        Line::from(vec![
            Span::styled("?        ", style_accent()),
            Span::from("Toggle help"),
        ]),
        Line::from(vec![
            Span::styled("/        ", style_accent()),
            Span::from("Search / filter"),
        ]),
        Line::from(vec![
            Span::styled("s        ", style_accent()),
            Span::from("Change sort column"),
        ]),
        Line::from(vec![
            Span::styled("S        ", style_accent()),
            Span::from("Toggle sort order"),
        ]),
        Line::from(vec![
            Span::styled("↑/↓     ", style_accent()),
            Span::from("Navigate processes"),
        ]),
        Line::from(vec![
            Span::styled("PgUp/Dn  ", style_accent()),
            Span::from("Page up/down"),
        ]),
        Line::from(vec![
            Span::styled("o        ", style_accent()),
            Span::from("Overview screen"),
        ]),
        Line::from(vec![
            Span::styled("k        ", style_accent()),
            Span::from("Kill selected process"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Sorting by: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("{} {}", app.sort_by.label(), if app.sort_desc { "↓" } else { "↑" }),
                style_accent(),
            ),
        ]),
    ];

    let para = Paragraph::new(Text::from(help_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .border_type(BorderType::Double),
        )
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn draw_overview(frame: &mut Frame, area: Rect, app: &App) {
    let total_gb = app.total_mem as f64 / 1024.0 / 1024.0 / 1024.0;
    let used_gb = app.used_mem as f64 / 1024.0 / 1024.0 / 1024.0;
    let mem_pct = if app.total_mem > 0 {
        (app.used_mem as f64 / app.total_mem as f64) * 100.0
    } else {
        0.0
    };

    let swap_total = app.total_swap as f64 / 1024.0 / 1024.0 / 1024.0;
    let swap_used = app.used_swap as f64 / 1024.0 / 1024.0 / 1024.0;
    let swap_pct = if app.total_swap > 0 {
        (app.used_swap as f64 / app.total_swap as f64) * 100.0
    } else {
        0.0
    };

    let days = System::uptime() / 86400;
    let hours = (System::uptime() % 86400) / 3600;
    let mins = (System::uptime() % 3600) / 60;
    let uptime_str = format!("{}d {:02}h {:02}m", days, hours, mins);

    let gpu_mem_str = if app.gpu_mem_total > 0 {
        format!("{:.1}/{:.1} GiB", app.gpu_mem_used as f64 / 1024.0, app.gpu_mem_total as f64 / 1024.0)
    } else {
        String::new()
    };
    let gpu_str = if app.gpu_usage > 0.0 {
        if gpu_mem_str.is_empty() {
            format!("{:.0}%", app.gpu_usage)
        } else {
            format!("{:.0}% | {}", app.gpu_usage, gpu_mem_str)
        }
    } else {
        "N/A".to_string()
    };

    let lines = vec![
        Line::from(vec![Span::styled(" Hostname ", style_accent()), Span::styled(&app.hostname, Style::default())]),
        Line::from(vec![Span::styled(" OS       ", style_accent()), Span::styled(&app.os_name, Style::default())]),
        Line::from(vec![Span::styled(" Kernel   ", style_accent()), Span::styled(&app.kernel, Style::default())]),
        Line::from(vec![Span::styled(" CPU      ", style_accent()), Span::styled(format!("{} ({} cores)", app.cpu_model, app.num_cpus), Style::default())]),
        Line::from(vec![Span::styled(" Memory   ", style_accent()), Span::styled(format!("{:.1} / {:.1} GiB ({:.0}%)", used_gb, total_gb, mem_pct), Style::default())]),
        Line::from(vec![Span::styled(" Swap     ", style_accent()), Span::styled(format!("{:.1} / {:.1} GiB ({:.0}%)", swap_used, swap_total, swap_pct), Style::default())]),
        Line::from(vec![Span::styled(" GPU      ", style_accent()), Span::styled(gpu_str, Style::default())]),
        Line::from(vec![Span::styled(" Uptime   ", style_accent()), Span::styled(uptime_str, Style::default())]),
        Line::from(vec![Span::styled(" Load Avg ", style_accent()), Span::styled(format!("{:.2}  {:.2}  {:.2}", app.load_one, app.load_five, app.load_fifteen), Style::default())]),
        Line::from(""),
        Line::from(vec![Span::styled(" [o] Processes  [q] Quit ", style_dim())]),
    ];

    let para = Paragraph::new(Text::from(lines))
        .block(Block::default().borders(Borders::ALL).title(" System Overview ").border_type(BorderType::Double))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn ui(frame: &mut Frame, app: &mut App) {
    match app.page {
        Page::Processes => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Min(1),
                ])
                .split(frame.area());

            draw_top_bar(frame, chunks[0], app);

            let search_area = chunks[1];
            draw_search_bar(frame, search_area, app);

            let table_area = chunks[2];
            draw_table(frame, table_area, app);

            if app.show_help {
                let help_area = centered_rect(50, 60, frame.area());
                draw_help(frame, help_area, app);
            }
        }
        Page::Overview => {
            draw_overview(frame, frame.area(), app);
        }
    }
}

fn run_app<B: Backend>(terminal: &mut ratatui::Terminal<B>, app: &mut App) -> io::Result<()> {
    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(1000);

    loop {
        terminal.draw(|f| ui(f, app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.search_focused {
                        match key.code {
                            KeyCode::Esc | KeyCode::Enter => {
                                app.search_focused = false;
                            }
                            KeyCode::Backspace => {
                                app.filter.pop();
                            }
                            KeyCode::Char(c) => {
                                if !c.is_control() {
                                    app.filter.push(c);
                                }
                            }
                            _ => {}
                        }
                    } else {
                        match app.page {
                            Page::Processes => {
                                match key.code {
                                    KeyCode::Char('q') => {
                                        app.should_quit = true;
                                    }
                                    KeyCode::Esc => {
                                        app.show_help = false;
                                    }
                                    KeyCode::Char('?') => {
                                        app.show_help = !app.show_help;
                                    }
                                    KeyCode::Char('/') => {
                                        app.search_focused = true;
                                    }
                                    KeyCode::Char('s') => {
                                        app.sort_desc = !app.sort_desc;
                                        app.sort();
                                    }
                                    KeyCode::Char('S') => {
                                        app.toggle_sort();
                                    }
                                    KeyCode::Char('o') => {
                                        app.page = Page::Overview;
                                    }
                                    KeyCode::Char('k') => {
                                        app.kill_selected();
                                    }
                                    KeyCode::Up => {
                                        app.prev();
                                    }
                                    KeyCode::Down => {
                                        app.next();
                                    }
                                    KeyCode::PageUp => {
                                        app.page_up();
                                    }
                                    KeyCode::PageDown => {
                                        app.page_down();
                                    }
                                    _ => {}
                                }
                            }
                            Page::Overview => {
                                match key.code {
                                    KeyCode::Char('q') => {
                                        app.should_quit = true;
                                    }
                                    KeyCode::Char('o') | KeyCode::Esc => {
                                        app.page = Page::Processes;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.refresh();
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = App::new();
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {}", err);
    }

    Ok(())
}
