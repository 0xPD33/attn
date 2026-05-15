use anyhow::Result;
#[cfg(test)]
use std::sync::Mutex;

pub enum NotificationKind {
    BreakOverdue { active_secs: i64, overdue_secs: i64 },
    BudgetExceeded { category: String, seconds: i64, budget_secs: i64 },
}

pub trait NotificationSink: Send + Sync {
    fn notify(&self, kind: NotificationKind) -> Result<()>;
}

pub struct DesktopNotifier {
    app_name: &'static str,
}

impl DesktopNotifier {
    pub fn new() -> Self {
        Self { app_name: "attn" }
    }
}

impl NotificationSink for DesktopNotifier {
    fn notify(&self, kind: NotificationKind) -> Result<()> {
        let (summary, body, urgency) = match &kind {
            NotificationKind::BreakOverdue { active_secs, overdue_secs } => {
                let active_min = active_secs / 60;
                let active_sec = active_secs % 60;
                let active_str = if active_min > 0 {
                    format!("{}m {}s", active_min, active_sec)
                } else {
                    format!("{}s", active_sec)
                };
                let summary = format!("{} — take a break", self.app_name);
                let body = format!("focused for {}, over by {}s", active_str, overdue_secs);
                (summary, body, notify_rust::Urgency::Normal)
            }
            NotificationKind::BudgetExceeded { category, seconds, budget_secs } => {
                let summary = format!(
                    "{} — {} budget reached ({}s / {}s)",
                    self.app_name, category, seconds, budget_secs
                );
                let body = String::new();
                (summary, body, notify_rust::Urgency::Low)
            }
        };

        let result = notify_rust::Notification::new()
            .appname(self.app_name)
            .summary(&summary)
            .body(&body)
            .urgency(urgency)
            .show();

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("desktop notification failed: {e}")),
        }
    }
}

pub struct NullNotifier;

impl NotificationSink for NullNotifier {
    fn notify(&self, _kind: NotificationKind) -> Result<()> {
        Ok(())
    }
}

/// A notification sink that records all calls for testing.
#[cfg(test)]
pub struct RecordingNotifier {
    pub calls: Mutex<Vec<String>>,
}

#[cfg(test)]
impl RecordingNotifier {
    pub fn new() -> Self {
        Self { calls: Mutex::new(Vec::new()) }
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[cfg(test)]
impl NotificationSink for RecordingNotifier {
    fn notify(&self, kind: NotificationKind) -> Result<()> {
        let label = match &kind {
            NotificationKind::BreakOverdue { .. } => "break_overdue".to_string(),
            NotificationKind::BudgetExceeded { category, .. } => {
                format!("budget_exceeded:{}", category)
            }
        };
        self.calls.lock().unwrap().push(label);
        Ok(())
    }
}
