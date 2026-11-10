use crate::compositor::Context;
use crate::job::{self, Callback};
use helix_view::editor::Severity;
use std::fmt::Write;

/// Show notification history
pub fn show_notification_history(cx: &mut Context) {
    if !cx.editor.config().notifications.enable_history {
        cx.editor
            .set_status("The notification history is off (`notifications.enable-history`)");
        return;
    }
    let history = cx.editor.get_notification_history();

    if history.is_empty() {
        cx.editor.set_status("No notifications in history");
        return;
    }

    // The newest first, at most `SHOWN` of them.
    const SHOWN: usize = 50;
    let mut content = if history.len() > SHOWN {
        format!(
            "Notification History (the {SHOWN} newest of {}):\n\n",
            history.len()
        )
    } else {
        "Notification History:\n\n".to_string()
    };

    for (i, notification) in history.iter().enumerate().rev().take(SHOWN) {
        let severity_icon = match notification.severity {
            Severity::Error => "❌",
            Severity::Warning => "⚠️",
            Severity::Info => "ℹ️",
            Severity::Hint => "💡",
        };

        let timestamp = notification.timestamp.elapsed().as_secs();
        let time_str = if timestamp < 60 {
            format!("{}s ago", timestamp)
        } else if timestamp < 3600 {
            format!("{}m ago", timestamp / 60)
        } else {
            format!("{}h ago", timestamp / 3600)
        };

        let _ = write!(
            content,
            "{:2}. {} {} ({})\n    {}\n\n",
            history.len() - i,
            severity_icon,
            time_str,
            // an expired notification is not shown any more, even before it is cleaned up
            if notification.is_active() {
                "active"
            } else {
                "dismissed"
            },
            notification.message
        );
    }

    let popup = crate::ui::Popup::new("notification-history", crate::ui::Text::new(content))
        .auto_close(true);

    cx.jobs.callback(async move {
        let call: job::Callback =
            Callback::EditorCompositor(Box::new(move |_editor, compositor| {
                compositor.push(Box::new(popup));
            }));
        Ok(call)
    });
}

/// Clear notification history
pub fn clear_notification_history(cx: &mut Context) {
    cx.editor.clear_notification_history();
    cx.editor.set_status("Notification history cleared");
}

/// Dismiss all active notifications
pub fn dismiss_all_notifications(cx: &mut Context) {
    cx.editor.dismiss_all_notifications();
    cx.editor.set_status("All notifications dismissed");
}

/// Test notification system with sample notifications
pub fn test_notifications(cx: &mut Context) {
    let timeout = cx.editor.config().notifications.default_timeout;
    let message = if timeout.is_zero() {
        "Test notification (no timeout: `:notifications-dismiss` removes it)".to_string()
    } else {
        format!(
            "Test notification (disappears after {}s)",
            timeout.as_secs_f64()
        )
    };
    cx.editor.notify_info(message);
}
