//! D-pad focus traversal (spec 6.3), shared by `webos`/`tizen` click and
//! focus actions. The algorithm runs against a [`FocusProbe`] so it is
//! unit-testable with a fake; the connector plugs in a probe backed by the
//! helper script.

use serde::Deserialize;

use impresario_core::ast::Direction;

use crate::errors::ConnectorError;

/// A CSS-pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Frame {
    #[must_use]
    pub fn center_x(&self) -> f64 {
        self.x + self.w / 2.0
    }

    #[must_use]
    pub fn center_y(&self) -> f64 {
        self.y + self.h / 2.0
    }

    /// Stable identity for cycle detection (rounded to whole pixels).
    #[must_use]
    pub fn hash(&self) -> String {
        format!("{:.0},{:.0},{:.0},{:.0}", self.x, self.y, self.w, self.h)
    }
}

/// What the probe reports about the currently focused element.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct FocusState {
    #[serde(default)]
    pub present: bool,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub frame: Option<Frame>,
}

/// The executor-side operations the traversal needs (spec 6.3).
#[async_trait::async_trait]
pub trait FocusProbe: Send + Sync {
    async fn current_focus(&self) -> Result<FocusState, ConnectorError>;
    /// `Some(frame)` while the target exists and is visible.
    async fn target_frame(&self) -> Result<Option<Frame>, ConnectorError>;
    async fn target_focused(&self) -> Result<bool, ConnectorError>;
    /// One D-pad press (`up`/`down`/`left`/`right`).
    async fn press(&self, direction: Direction) -> Result<(), ConnectorError>;
}

/// One entry of `details.focus_trail` (spec 6.3).
#[derive(Debug, Clone, PartialEq)]
pub struct TrailEntry {
    pub step: u32,
    pub direction: &'static str,
    pub focused_label: String,
    pub frame: Option<Frame>,
}

impl TrailEntry {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "step": self.step,
            "direction": self.direction,
            "focused_label": self.focused_label,
            "frame": self.frame.map(|frame| serde_json::json!({
                "x": frame.x,
                "y": frame.y,
                "w": frame.w,
                "h": frame.h,
            })),
        })
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Up => "up",
        Direction::Down => "down",
        Direction::Left => "left",
        Direction::Right => "right",
    }
}

fn focus_error(reason: &str, trail: &[TrailEntry]) -> ConnectorError {
    let json = serde_json::Value::Array(trail.iter().map(TrailEntry::to_json).collect());
    ConnectorError::focus_unreachable(format!(
        "the D-pad traversal did not reach the target ({reason})"
    ))
    .with_detail("reason", reason)
    .with_detail("steps", trail.len().to_string())
    .with_detail("focus_trail", json.to_string())
}

/// The traversal algorithm of spec 6.3: dominant-axis D-pad presses with
/// cycle detection and the other-axis retry on a blocked move.
pub async fn navigate(probe: &dyn FocusProbe, max_steps: u32) -> Result<(), ConnectorError> {
    let mut seen: Vec<(String, &'static str)> = Vec::new();
    let mut trail: Vec<TrailEntry> = Vec::new();
    let mut last_blocked = false;

    if probe.target_focused().await? {
        return Ok(());
    }

    for step in 1..=max_steps {
        if probe.target_focused().await? {
            return Ok(());
        }
        let Some(target) = probe.target_frame().await? else {
            return Err(focus_error("budget", &trail)
                .with_detail("message", "the target disappeared during traversal"));
        };
        let focused = probe.current_focus().await?;
        let Some(frame) = focused.frame else {
            probe.press(Direction::Down).await?;
            trail.push(TrailEntry {
                step,
                direction: "down",
                focused_label: focused.label.clone(),
                frame: None,
            });
            last_blocked = false;
            continue;
        };

        let dx = target.center_x() - frame.center_x();
        let dy = target.center_y() - frame.center_y();
        let primary = if dy.abs() >= dx.abs() {
            if dy >= 0.0 {
                Direction::Down
            } else {
                Direction::Up
            }
        } else if dx >= 0.0 {
            Direction::Right
        } else {
            Direction::Left
        };
        let primary_name = direction_name(primary);
        if seen.contains(&(frame.hash(), primary_name)) {
            return Err(focus_error("cycle", &trail));
        }
        seen.push((frame.hash(), primary_name));
        probe.press(primary).await?;
        trail.push(TrailEntry {
            step,
            direction: primary_name,
            focused_label: focused.label.clone(),
            frame: Some(frame),
        });
        if probe.target_focused().await? {
            return Ok(());
        }
        let after = probe.current_focus().await?;
        let moved = after.frame.as_ref().map(Frame::hash) != Some(frame.hash());
        if moved {
            last_blocked = false;
            continue;
        }

        // The primary press did not change focus: try the other axis once.
        let secondary = match primary {
            Direction::Up | Direction::Down => {
                if dx >= 0.0 {
                    Direction::Right
                } else {
                    Direction::Left
                }
            }
            Direction::Left | Direction::Right => {
                if dy >= 0.0 {
                    Direction::Down
                } else {
                    Direction::Up
                }
            }
        };
        let secondary_name = direction_name(secondary);
        if seen.contains(&(frame.hash(), secondary_name)) {
            return Err(focus_error("cycle", &trail));
        }
        seen.push((frame.hash(), secondary_name));
        probe.press(secondary).await?;
        trail.push(TrailEntry {
            step,
            direction: secondary_name,
            focused_label: focused.label.clone(),
            frame: Some(frame),
        });
        if probe.target_focused().await? {
            return Ok(());
        }
        let after_secondary = probe.current_focus().await?;
        let moved_secondary = after_secondary.frame.as_ref().map(Frame::hash) != Some(frame.hash());
        if !moved_secondary {
            last_blocked = true;
            trail.push(TrailEntry {
                step,
                direction: "blocked",
                focused_label: after_secondary.label,
                frame: after_secondary.frame,
            });
        } else {
            last_blocked = false;
        }
    }

    Err(focus_error(
        if last_blocked { "blocked" } else { "budget" },
        &trail,
    ))
}

#[cfg(test)]
pub(crate) mod fake {
    use std::sync::Mutex;

    use super::*;

    /// A scripted focus grid: nodes at `(x, y)` pixels; presses move focus by
    /// the first candidate in the pressed direction, or do nothing.
    pub struct FakeProbe {
        pub nodes: Vec<(String, Frame)>,
        pub focused: Mutex<usize>,
        pub target: usize,
        pub lockable: Vec<&'static str>,
    }

    impl FakeProbe {
        pub fn new(nodes: Vec<(String, Frame)>, target: usize) -> Self {
            Self {
                nodes,
                focused: Mutex::new(0),
                target,
                lockable: Vec::new(),
            }
        }

        pub fn current_label(&self) -> String {
            let index = *self.focused.lock().expect("focus lock");
            self.nodes
                .get(index)
                .map(|(label, _)| label.clone())
                .unwrap_or_default()
        }

        fn center(&self, index: usize) -> (f64, f64) {
            self.nodes.get(index).map_or((0.0, 0.0), |(_, frame)| {
                (frame.center_x(), frame.center_y())
            })
        }
    }

    #[async_trait::async_trait]
    impl FocusProbe for FakeProbe {
        async fn current_focus(&self) -> Result<FocusState, ConnectorError> {
            let index = *self.focused.lock().expect("focus lock");
            let (label, frame) = self
                .nodes
                .get(index)
                .map(|(label, frame)| (label.clone(), *frame))
                .unwrap_or_else(|| {
                    (
                        String::new(),
                        Frame {
                            x: 0.0,
                            y: 0.0,
                            w: 0.0,
                            h: 0.0,
                        },
                    )
                });
            Ok(FocusState {
                present: true,
                label,
                frame: Some(frame),
            })
        }

        async fn target_frame(&self) -> Result<Option<Frame>, ConnectorError> {
            Ok(self.nodes.get(self.target).map(|(_, frame)| *frame))
        }

        async fn target_focused(&self) -> Result<bool, ConnectorError> {
            Ok(*self.focused.lock().expect("focus lock") == self.target)
        }

        async fn press(&self, direction: Direction) -> Result<(), ConnectorError> {
            let mut focused = self.focused.lock().expect("focus lock");
            if self.lockable.contains(&direction_name(direction)) {
                return Ok(());
            }
            let (cx, cy) = self.center(*focused);
            let mut best: Option<(usize, f64)> = None;
            for (index, (_, frame)) in self.nodes.iter().enumerate() {
                if index == *focused {
                    continue;
                }
                let (tx, ty) = (frame.center_x(), frame.center_y());
                let in_direction = match direction {
                    Direction::Up => ty < cy,
                    Direction::Down => ty > cy,
                    Direction::Left => tx < cx,
                    Direction::Right => tx > cx,
                };
                if !in_direction {
                    continue;
                }
                let distance = (tx - cx).abs() + (ty - cy).abs();
                if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                    best = Some((index, distance));
                }
            }
            if let Some((index, _)) = best {
                *focused = index;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeProbe;
    use super::*;

    fn frame(x: f64, y: f64) -> Frame {
        Frame {
            x,
            y,
            w: 100.0,
            h: 40.0,
        }
    }

    fn grid() -> FakeProbe {
        FakeProbe::new(
            vec![
                ("a".to_string(), frame(0.0, 0.0)),
                ("b".to_string(), frame(0.0, 100.0)),
                ("c".to_string(), frame(0.0, 200.0)),
            ],
            2,
        )
    }

    #[tokio::test]
    async fn traversal_reaches_a_lower_target() {
        let probe = grid();
        navigate(&probe, 50).await.expect("reachable");
        assert_eq!(probe.current_label(), "c");
    }

    #[tokio::test]
    async fn budget_failure_reports_the_trail() {
        let probe = grid();
        let error = navigate(&probe, 1).await.expect_err("budget");
        assert_eq!(
            error.details.get("reason").map(String::as_str),
            Some("budget")
        );
        assert_eq!(error.details.get("steps").map(String::as_str), Some("1"));
        assert!(error
            .details
            .get("focus_trail")
            .is_some_and(|trail| trail.starts_with('[')));
    }

    /// Toggles focus between two nodes whatever is pressed, so the
    /// `(frame hash, direction)` pair repeats and the cycle guard fires.
    struct WrapProbe {
        inner: FakeProbe,
    }

    #[async_trait::async_trait]
    impl FocusProbe for WrapProbe {
        async fn current_focus(&self) -> Result<FocusState, ConnectorError> {
            self.inner.current_focus().await
        }

        async fn target_frame(&self) -> Result<Option<Frame>, ConnectorError> {
            self.inner.target_frame().await
        }

        async fn target_focused(&self) -> Result<bool, ConnectorError> {
            self.inner.target_focused().await
        }

        async fn press(&self, _direction: Direction) -> Result<(), ConnectorError> {
            let mut focused = self.inner.focused.lock().expect("focus lock");
            *focused = if *focused == 0 { 1 } else { 0 };
            Ok(())
        }
    }

    #[tokio::test]
    async fn cycle_is_detected() {
        let probe = WrapProbe { inner: grid() };
        let error = navigate(&probe, 50).await.expect_err("cycle");
        assert_eq!(
            error.details.get("reason").map(String::as_str),
            Some("cycle")
        );
        assert!(error
            .details
            .get("focus_trail")
            .is_some_and(|trail| trail.starts_with('[')));
    }

    #[tokio::test]
    async fn blocked_presses_land_in_the_trail() {
        let probe = FakeProbe {
            lockable: vec!["up", "down", "left", "right"],
            ..grid()
        };
        let error = navigate(&probe, 2).await.expect_err("stuck");
        assert!(error
            .details
            .get("focus_trail")
            .is_some_and(|trail| trail.contains("\"blocked\"")));
    }

    #[tokio::test]
    async fn already_focused_target_is_instant() {
        let probe = FakeProbe::new(vec![("a".to_string(), frame(0.0, 0.0))], 0);
        navigate(&probe, 0).await.expect("already focused");
    }
}
