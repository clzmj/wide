//! Pure layout decoding and planning. Commands here are candidates; the application
//! must enforce its hardware-verification gate before submitting any step.
use crate::{protocol::Reading, resolution::ResolutionRole};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Hdmi1,
    DisplayPort,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hdmi1 => "HDMI1",
            Self::DisplayPort => "DisplayPort",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Hdmi1 => Self::DisplayPort,
            Self::DisplayPort => Self::Hdmi1,
        }
    }

    pub fn from_wire(value: u8) -> Result<Self, String> {
        match value {
            0x00 => Ok(Self::Hdmi1),
            0x10 => Ok(Self::DisplayPort),
            _ => Err("Only HDMI1 and DisplayPort source pairs are supported".into()),
        }
    }

    pub fn from_input_vcp(value: u16) -> Result<Self, String> {
        match value {
            0x11 => Ok(Self::Hdmi1),
            0x0f => Ok(Self::DisplayPort),
            _ => Err("Current input is not HDMI1 or DisplayPort".into()),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for Source {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "hdmi1" | "hdmi-1" => Ok(Self::Hdmi1),
            "displayport" | "display-port" | "display_port" | "dp" => Ok(Self::DisplayPort),
            _ => Err("Choose HDMI1 or DisplayPort".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutChoice {
    Fullscreen,
    SplitLeft,
    SplitRight,
    PipBig,
    PipSmall,
}

impl LayoutChoice {
    pub fn resolution_role(self) -> ResolutionRole {
        match self {
            Self::Fullscreen => ResolutionRole::Fullscreen,
            Self::SplitLeft | Self::SplitRight => ResolutionRole::SplitHalf,
            Self::PipBig => ResolutionRole::PipMain,
            Self::PipSmall => ResolutionRole::PipInset,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Fullscreen => "Full-screen",
            Self::SplitLeft => "Split view · this Mac on the left",
            Self::SplitRight => "Split view · this Mac on the right",
            Self::PipBig => "Picture in picture · this Mac in the big picture",
            Self::PipSmall => "Picture in picture · this Mac in the small picture",
        }
    }
}

impl fmt::Display for LayoutChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Fullscreen => "fullscreen",
            Self::SplitLeft => "split-left",
            Self::SplitRight => "split-right",
            Self::PipBig => "pip-big",
            Self::PipSmall => "pip-small",
        })
    }
}

impl FromStr for LayoutChoice {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().replace('_', "-").as_str() {
            "fullscreen" | "full-screen" => Ok(Self::Fullscreen),
            "split-left" => Ok(Self::SplitLeft),
            "split-right" => Ok(Self::SplitRight),
            "pip-big" => Ok(Self::PipBig),
            "pip-small" => Ok(Self::PipSmall),
            _ => Err("Choose fullscreen, split-left, split-right, pip-big, or pip-small".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorState {
    pub enabled: bool,
    pub mode: u8,
    pub main: Source,
    pub secondary: Source,
    /// Observed monitor input; this may follow main-picture changes, not this Mac.
    pub input: Option<Source>,
    pub raw_e2: u16,
}

impl MonitorState {
    pub fn decode(e2: &Reading, e3: &Reading, input: &Reading) -> Result<Self, String> {
        let raw_e2 = current(e2, "layout")?;
        if e2.max != Some(0x7f) || raw_e2 >> 8 != 0x35 {
            return Err("Monitor layout reply does not match the supported G93SC profile".into());
        }
        let pair = current(e3, "source pair")?;
        if e3.max != Some(0x5151) {
            return Err("Monitor source reply does not match the supported G93SC profile".into());
        }
        let state = Self {
            enabled: raw_e2 & 1 != 0,
            mode: ((raw_e2 >> 1) & 7) as u8,
            main: Source::from_wire((pair >> 8) as u8)?,
            secondary: Source::from_wire(pair as u8)?,
            input: if input.status == "ok" {
                Some(Source::from_input_vcp(current(input, "current input")?)?)
            } else {
                None
            },
            raw_e2,
        };
        state.validate()?;
        Ok(state)
    }

    fn validate(&self) -> Result<(), String> {
        if self.main == self.secondary {
            return Err("HDMI1 and DisplayPort must be distinct picture sources".into());
        }
        if !matches!(self.mode, 0..=2 | 7) {
            return Err("Monitor reports an unsupported layout".into());
        }
        if self.raw_e2 >> 8 != 0x35
            || self.enabled != (self.raw_e2 & 1 != 0)
            || self.mode != ((self.raw_e2 >> 1) & 7) as u8
        {
            return Err("Monitor layout state is inconsistent with its reply".into());
        }
        Ok(())
    }

    pub fn label(&self) -> &'static str {
        if !self.enabled {
            return "Full-screen";
        }
        match self.mode {
            0 => "Split view · equal halves",
            1 | 2 => "Split view · unequal halves",
            7 => "Picture in picture",
            _ => "Unknown layout",
        }
    }
}

fn current(reading: &Reading, name: &str) -> Result<u16, String> {
    if reading.status != "ok" {
        return Err(format!("Cannot read monitor {name}: {}", reading.status));
    }
    reading
        .current
        .ok_or_else(|| format!("Monitor {name} reply has no current value"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Enable,
    Disable,
    RestorePbp,
    EqualSplit,
    SelectPip,
    SwapSources,
}

impl Step {
    pub fn command(self) -> (u8, u16) {
        match self {
            Self::Enable => (0xe2, 1),
            Self::Disable => (0xe2, 0),
            Self::RestorePbp => (0xe2, 2),
            Self::EqualSplit => (0xe2, 3),
            Self::SelectPip => (0xe2, 0x10),
            Self::SwapSources => (0xe3, 0x1000),
        }
    }

    pub fn verify(self, before: &MonitorState, after: &MonitorState) -> bool {
        if before.validate().is_err() || after.validate().is_err() {
            return false;
        }
        // The observed input may legitimately follow main-picture changes.
        // The host-port anchor is determined independently by the application.
        if self == Self::SwapSources {
            return before.enabled
                && after.enabled
                && after.main == before.secondary
                && after.secondary == before.main
                && after.raw_e2 == before.raw_e2;
        }
        if before.main != after.main || before.secondary != after.secondary {
            return false;
        }
        let changed_mask = match self {
            Self::Enable | Self::Disable => 1,
            Self::RestorePbp | Self::EqualSplit | Self::SelectPip => 0x0e,
            Self::SwapSources => unreachable!(),
        };
        if (before.raw_e2 ^ after.raw_e2) & !changed_mask != 0 {
            return false;
        }
        match self {
            Self::Enable => after.enabled && after.mode == before.mode,
            Self::Disable => !after.enabled && after.mode == before.mode,
            Self::RestorePbp => after.enabled == before.enabled && after.mode <= 2,
            Self::EqualSplit => after.enabled == before.enabled && after.mode == 0,
            Self::SelectPip => after.enabled == before.enabled && after.mode == 7,
            Self::SwapSources => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub before: MonitorState,
    pub choice: LayoutChoice,
    pub this_source: Source,
    pub target_main: Source,
    pub target_secondary: Source,
    pub resolution_role: ResolutionRole,
    pub steps: Vec<Step>,
}

impl Plan {
    pub fn build(
        choice: LayoutChoice,
        this_source: Source,
        state: &MonitorState,
    ) -> Result<Self, String> {
        state.validate()?;
        let target_main = match choice {
            LayoutChoice::Fullscreen | LayoutChoice::SplitLeft | LayoutChoice::PipBig => {
                this_source
            }
            LayoutChoice::SplitRight | LayoutChoice::PipSmall => this_source.other(),
        };
        let resolution_role = choice.resolution_role();
        let mut steps = Vec::new();
        let mut enabled = state.enabled;
        match choice {
            LayoutChoice::Fullscreen => {
                if state.main != target_main {
                    if !enabled {
                        steps.push(Step::Enable);
                        enabled = true;
                    }
                    steps.push(Step::SwapSources);
                }
                if enabled {
                    steps.push(Step::Disable);
                }
            }
            LayoutChoice::SplitLeft | LayoutChoice::SplitRight => {
                if state.mode == 7 {
                    steps.extend([Step::RestorePbp, Step::EqualSplit]);
                } else if state.mode != 0 {
                    steps.push(Step::EqualSplit);
                }
                if !enabled {
                    steps.push(Step::Enable);
                }
                if state.main != target_main {
                    steps.push(Step::SwapSources);
                }
            }
            LayoutChoice::PipBig | LayoutChoice::PipSmall => {
                if state.mode != 7 {
                    steps.push(Step::SelectPip);
                }
                if !enabled {
                    steps.push(Step::Enable);
                }
                if state.main != target_main {
                    steps.push(Step::SwapSources);
                }
            }
        }
        Ok(Self {
            before: *state,
            choice,
            this_source,
            target_main,
            target_secondary: target_main.other(),
            resolution_role,
            steps,
        })
    }

    pub fn summary(&self) -> String {
        let role = match self.choice {
            LayoutChoice::Fullscreen => "full-screen",
            LayoutChoice::SplitLeft => "left half",
            LayoutChoice::SplitRight => "right half",
            LayoutChoice::PipBig => "big picture (background)",
            LayoutChoice::PipSmall => "small picture (inset)",
        };
        let pair = match self.choice {
            LayoutChoice::Fullscreen => format!("Full-screen picture: {}", self.target_main),
            LayoutChoice::SplitLeft | LayoutChoice::SplitRight => format!(
                "Left: {} · Right: {}",
                self.target_main, self.target_secondary
            ),
            LayoutChoice::PipBig | LayoutChoice::PipSmall => format!(
                "Big picture: {} · Small picture: {}",
                self.target_main, self.target_secondary
            ),
        };
        format!(
            "{}\nThis Mac: {} · {role}\n{pair}",
            match self.choice {
                LayoutChoice::Fullscreen => "Full-screen",
                LayoutChoice::SplitLeft | LayoutChoice::SplitRight => "Split view",
                _ => "Picture in picture",
            },
            self.this_source
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(enabled: bool, mode: u8, main: Source) -> MonitorState {
        MonitorState {
            enabled,
            mode,
            main,
            secondary: main.other(),
            input: Some(Source::Hdmi1),
            raw_e2: 0x35c0 | (u16::from(mode) << 1) | u16::from(enabled),
        }
    }

    fn reading(current: u16, max: u16) -> Reading {
        Reading {
            status: "ok".into(),
            current: Some(current),
            max: Some(max),
            result_code: Some(0),
            frame_offset: Some(0),
        }
    }

    #[test]
    fn decodes_the_recorded_profile_and_rejects_other_source_or_fingerprints() {
        let decoded = MonitorState::decode(
            &reading(0x35cf, 0x7f),
            &reading(0x0010, 0x5151),
            &reading(0x11, 0x12),
        )
        .unwrap();
        assert!(decoded.enabled);
        assert_eq!(decoded.mode, 7);
        assert_eq!(
            (decoded.main, decoded.secondary),
            (Source::Hdmi1, Source::DisplayPort)
        );
        for pair in [0x0110, 0x0000, 0x0011] {
            assert!(MonitorState::decode(
                &reading(0x35cf, 0x7f),
                &reading(pair, 0x5151),
                &reading(0x11, 0x12)
            )
            .is_err());
        }
        for (e2, maximum) in [(0x34cf, 0x7f), (0x35cf, 0xff), (0x35c7, 0x7f)] {
            assert!(MonitorState::decode(
                &reading(e2, maximum),
                &reading(0x0010, 0x5151),
                &reading(0x11, 0x12)
            )
            .is_err());
        }
        assert!(MonitorState::decode(
            &reading(0x35cf, 0x7f),
            &reading(0x0010, 0xffff),
            &reading(0x11, 0x12)
        )
        .is_err());
        assert!(MonitorState::decode(
            &reading(0x35cf, 0x7f),
            &reading(0x0010, 0x5151),
            &reading(0x12, 0x12)
        )
        .is_err());
        assert!(Source::from_str("HDMI2").is_err());
    }

    #[test]
    fn all_choices_keep_this_mac_in_the_requested_role_for_either_port() {
        for this_source in [Source::Hdmi1, Source::DisplayPort] {
            for choice in [
                LayoutChoice::Fullscreen,
                LayoutChoice::SplitLeft,
                LayoutChoice::SplitRight,
                LayoutChoice::PipBig,
                LayoutChoice::PipSmall,
            ] {
                let plan =
                    Plan::build(choice, this_source, &state(false, 7, Source::Hdmi1)).unwrap();
                let main = matches!(
                    choice,
                    LayoutChoice::Fullscreen | LayoutChoice::SplitLeft | LayoutChoice::PipBig
                );
                assert_eq!(plan.target_main == this_source, main);
                assert_eq!(plan.target_secondary, plan.target_main.other());
                assert!(!plan.summary().contains("VCP"));
                assert_eq!(choice.to_string().parse::<LayoutChoice>(), Ok(choice));
                assert!(plan.steps.iter().all(|step| step.command().0 != 0x60));
            }
        }
    }

    #[test]
    fn full_screen_swaps_while_enabled_before_disabling() {
        assert_eq!(
            Plan::build(
                LayoutChoice::Fullscreen,
                Source::DisplayPort,
                &state(false, 7, Source::Hdmi1)
            )
            .unwrap()
            .steps,
            [Step::Enable, Step::SwapSources, Step::Disable]
        );
        assert_eq!(
            Plan::build(
                LayoutChoice::Fullscreen,
                Source::DisplayPort,
                &state(true, 7, Source::Hdmi1)
            )
            .unwrap()
            .steps,
            [Step::SwapSources, Step::Disable]
        );
        assert_eq!(
            Plan::build(
                LayoutChoice::SplitLeft,
                Source::Hdmi1,
                &state(false, 7, Source::DisplayPort)
            )
            .unwrap()
            .steps,
            [
                Step::RestorePbp,
                Step::EqualSplit,
                Step::Enable,
                Step::SwapSources
            ]
        );
        assert_eq!(
            Plan::build(
                LayoutChoice::PipSmall,
                Source::Hdmi1,
                &state(false, 0, Source::Hdmi1)
            )
            .unwrap()
            .steps,
            [Step::SelectPip, Step::Enable, Step::SwapSources]
        );
    }

    #[test]
    fn matching_layouts_need_no_setting_steps() {
        for this_source in [Source::Hdmi1, Source::DisplayPort] {
            for (choice, enabled, mode, main) in [
                (LayoutChoice::Fullscreen, false, 7, this_source),
                (LayoutChoice::SplitLeft, true, 0, this_source),
                (LayoutChoice::SplitRight, true, 0, this_source.other()),
                (LayoutChoice::PipBig, true, 7, this_source),
                (LayoutChoice::PipSmall, true, 7, this_source.other()),
            ] {
                assert!(
                    Plan::build(choice, this_source, &state(enabled, mode, main))
                        .unwrap()
                        .steps
                        .is_empty()
                );
            }
        }
    }

    #[test]
    fn step_verification_preserves_unrelated_layout_and_pair_fields() {
        let off = state(false, 7, Source::Hdmi1);
        let on = state(true, 7, Source::Hdmi1);
        assert!(Step::Enable.verify(&off, &on));
        assert!(Step::Disable.verify(&on, &off));
        let mut swapped = state(true, 7, Source::DisplayPort);
        swapped.input = Some(Source::DisplayPort);
        assert!(Step::SwapSources.verify(&on, &swapped));
        assert!(!Step::SwapSources.verify(&off, &state(false, 7, Source::DisplayPort)));
        assert!(Step::SelectPip.verify(&state(false, 0, Source::Hdmi1), &off));
        for mode in 0..=2 {
            assert!(Step::RestorePbp.verify(&on, &state(true, mode, Source::Hdmi1)));
        }
        assert!(Step::EqualSplit.verify(
            &state(true, 2, Source::Hdmi1),
            &state(true, 0, Source::Hdmi1)
        ));
        let mut unrelated_change = on;
        unrelated_change.raw_e2 ^= 0x10;
        assert!(!Step::Enable.verify(&off, &unrelated_change));
        assert!(!Step::Enable.verify(&off, &swapped));
        assert!(!Step::RestorePbp.verify(&on, &state(false, 0, Source::Hdmi1)));
    }
}
