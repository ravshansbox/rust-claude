use super::{App, Request, Role};
use crate::settings::Settings;
use tokio::sync::mpsc;

pub(super) fn send_changed_effort(
    app: &App,
    sent_effort: &mut &'static str,
    requests: &mpsc::UnboundedSender<Request>,
) {
    if app.effort != *sent_effort {
        *sent_effort = app.effort;
        let _ = requests.send(Request::SetEffort(app.effort));
    }
}

pub(super) fn save_changed_settings(
    app: &mut App,
    saved_model: &mut String,
    saved_effort: &mut &'static str,
) {
    if app.model == *saved_model && app.effort == *saved_effort {
        return;
    }
    let model = (app.model != *saved_model).then(|| app.model.clone());
    let effort = (app.effort != *saved_effort).then_some(app.effort);
    saved_model.clone_from(&app.model);
    *saved_effort = app.effort;
    let saved = Settings::update(|settings| {
        if let Some(model) = model {
            settings.model = Some(model);
        }
        if let Some(effort) = effort {
            settings.effort = Some(effort.to_string());
        }
    });
    if let Err(error) = saved {
        app.push(Role::Event, format!("failed to save settings: {error:#}"));
    }
}
