mod collector;
mod daemon;
mod metrics;
mod procfs;
mod save;
mod ui;

use std::sync::Arc;
use std::time::Duration;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};

use collector::{Collector, Control};

#[derive(Parser, Debug)]
#[command(name = "server-spy", version, about = "System congestion tracker: daemon + TUI")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// attach the TUI to the daemon (starts it if needed)
    Tui {
        #[command(flatten)]
        common: Common,
    },
    /// start the background daemon
    Start {
        #[command(flatten)]
        common: Common,
    },
    /// stop the daemon; its collected data is discarded
    Stop,
    /// run the daemon in the foreground (debugging)
    Daemon {
        #[command(flatten)]
        common: Common,
        /// double-fork and detach from the terminal
        #[arg(long)]
        detach: bool,
    },
    /// print a snapshot as text and exit
    Dump {
        #[command(flatten)]
        common: Common,
        /// how many polls to collect in standalone mode
        #[arg(long, default_value_t = 3)]
        count: usize,
        /// read the snapshot from the running daemon instead
        #[arg(long)]
        via_daemon: bool,
    },
}

#[derive(Args, Debug, Default)]
struct Common {
    /// poll interval in seconds
    #[arg(short, long, default_value_t = 1.0, value_name = "SECS")]
    interval: f64,
    /// target process name
    #[arg(long, default_value = "", value_name = "NAME")]
    target: String,
    /// history samples kept for sparklines (30 min at 1s interval)
    #[arg(long, default_value_t = 1800)]
    history: usize,
}

fn dur(secs: f64) -> Duration {
    Duration::from_secs_f64(secs.max(0.1))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let exe = daemon::exe_name();
    let matches = Cli::command().name(exe).get_matches();
    let cli = Cli::from_arg_matches(&matches)?;
    match cli.cmd.unwrap_or(Cmd::Tui {
        common: Common::default(),
    }) {
        Cmd::Tui { common } => {
            daemon::ensure(&common.target, dur(common.interval), common.history)?;
            let mut app = ui::App::new(common.target, common.history, dur(common.interval));
            ui::run(&mut app)?;
        }
        Cmd::Start { common } => {
            daemon::start(&common.target, dur(common.interval), common.history)?;
            println!("server-spy daemon started");
        }
        Cmd::Stop => {
            if daemon::is_running() {
                daemon::stop()?;
                println!("server-spy daemon stopped");
            } else {
                println!("no daemon running");
            }
        }
        Cmd::Daemon { common, detach } => {
            if detach {
                daemon::run_detached(common.target, dur(common.interval), common.history);
            } else {
                daemon::run_foreground(common.target, dur(common.interval), common.history)?;
            }
        }
        Cmd::Dump {
            common,
            count,
            via_daemon,
        } => {
            if via_daemon {
                if let Some(snap) = daemon::request_snapshot(0)? {
                    let sys = procfs::SysInfo::detect();
                    print!("{}", collector::snapshot_text(&snap, &sys));
                }
            } else {
                let control = Arc::new(Control::new(common.target.clone()));
                let mut col = Collector::new(dur(common.interval), control, common.history);
                let mut snap = None;
                for i in 0..count {
                    if i > 0 {
                        std::thread::sleep(dur(common.interval));
                    }
                    snap = Some(col.poll());
                }
                if let Some(s) = snap {
                    print!("{}", collector::snapshot_text(&s, &col.sys));
                }
            }
        }
    }
    Ok(())
}
