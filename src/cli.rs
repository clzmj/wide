use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "wide",
    version,
    about = "Choose how this Mac appears on your Samsung ultrawide",
    after_help = "Run wide without a command to choose a layout interactively."
)]
pub struct Cli {
    #[arg(long, global = true, hide = true)]
    pub json: bool,
    /// Registry ID (decimal or 0xHEX), or unique display-name substring.
    #[arg(long, global = true, hide = true)]
    pub display: Option<String>,
    /// Offline fixture; never opens a physical display.
    #[arg(long, global = true, hide = true)]
    pub mock: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Show the connected monitor and current layout.
    Status,
    /// Advanced diagnostics and explicitly gated monitor controls.
    Advanced {
        #[command(subcommand)]
        command: AdvancedCommand,
    },
}

#[derive(Subcommand)]
pub enum AdvancedCommand {
    /// Refresh this Mac's video timing using one advertised mode; no DDC settings.
    Reconnect {
        /// Explicit resolution target; omission preserves the current mode.
        #[arg(long)]
        layout: Option<String>,
        #[arg(long)]
        confirm: bool,
        /// Briefly deactivate/reactivate this connection; requires another active display.
        #[arg(long)]
        cycle: bool,
    },
    /// Preview the automatic monitor-audio rule; --confirm applies only sound.
    Sound {
        #[arg(long)]
        confirm: bool,
    },
    /// Preview a layout and its local resolution policy without changing anything.
    Plan {
        layout: String,
        #[arg(long)]
        port: Option<String>,
    },
    /// Apply a confirmed plan. The experimental flag is only for reviewed validation runs.
    Apply {
        layout: String,
        #[arg(long)]
        port: Option<String>,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        log: Option<PathBuf>,
        #[cfg(feature = "experimental-writes")]
        #[arg(long)]
        experimental_layout: bool,
    },
    /// List this Mac's advertised display modes without changing them.
    Modes,
    List,
    Info,
    Capabilities,
    Diagnose {
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = "unlabelled")]
        label: String,
        #[arg(long)]
        all: bool,
    },
    Ddc {
        #[command(subcommand)]
        command: Ddc,
    },
    Input {
        source: String,
    },
    Mode {
        #[arg(value_parser = ["fullscreen", "pbp", "pip"])]
        mode: String,
        /// Permit the narrowly verified undocumented G93SC enable command.
        #[arg(long)]
        allow_undocumented: bool,
        /// Confirm one setting write; physical visual verification is still required.
        #[arg(long)]
        confirm: bool,
    },
    Pbp {
        #[arg(long)]
        left: String,
        #[arg(long)]
        right: String,
    },
    Pip {
        #[arg(long)]
        main: String,
        #[arg(long)]
        secondary: String,
    },
    Preset {
        #[command(subcommand)]
        command: Preset,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_command_opens_form_and_diagnostics_are_nested() {
        assert!(Cli::try_parse_from(["wide"]).unwrap().command.is_none());
        assert!(matches!(
            Cli::try_parse_from(["wide", "status"]).unwrap().command,
            Some(Command::Status)
        ));
        assert!(matches!(
            Cli::try_parse_from(["wide", "advanced", "ddc", "read", "0x60"])
                .unwrap()
                .command,
            Some(Command::Advanced {
                command: AdvancedCommand::Ddc {
                    command: Ddc::Read { .. }
                }
            })
        ));
        assert!(Cli::try_parse_from(["wide", "ddc", "read", "0x60"]).is_err());
    }

    #[test]
    fn reconnect_does_not_choose_a_layout_implicitly() {
        assert!(matches!(
            Cli::try_parse_from(["wide", "advanced", "reconnect", "--confirm"])
                .unwrap()
                .command,
            Some(Command::Advanced {
                command: AdvancedCommand::Reconnect {
                    layout: None,
                    confirm: true,
                    cycle: false
                }
            })
        ));
    }
}

#[derive(Subcommand)]
pub enum Ddc {
    Read {
        vcp: String,
    },
    /// Only E2=1 is enabled, with explicit flags and a matching G93SC PiP profile.
    Write {
        vcp: String,
        value: String,
        #[arg(long)]
        allow_undocumented: bool,
        #[arg(long)]
        confirm: bool,
    },
    Scan {
        #[arg(long, required = true)]
        read_only: bool,
        /// All 256 Get VCP codes, rather than common + vendor codes.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum Preset {
    /// Save an observed register snapshot; this is not an executable preset.
    Save {
        name: String,
    },
    Apply {
        name: String,
    },
}
