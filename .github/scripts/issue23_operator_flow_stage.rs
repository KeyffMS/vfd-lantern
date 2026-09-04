use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}:\n{}", path.display(), old);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn write(path: &str, content: &str) {
    fs::write(path, content).unwrap_or_else(|e| panic!("write {path}: {e}"));
}

fn main() {
    // Expose only immutable operator-facing write state. The SessionStateMachine remains the SPoT.
    replace_once(
        "crates/lantern-app/src/application.rs",
        "    authorization: AuthorizationView,\n    audit_health: AuditHealthView,\n    operation: OperationView,\n",
        "    authorization: AuthorizationView,\n    arming_challenge: Option<String>,\n    arming_expires_at: Option<Instant>,\n    armed_idle_expires_at: Option<Instant>,\n    audit_health: AuditHealthView,\n    operation: OperationView,\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "                authorization: match &active.authorization {\n                    Authorization::ProcessDisabled => AuthorizationView::ProcessDisabled,\n                    Authorization::Disarmed { .. } => AuthorizationView::Disarmed,\n                    Authorization::Arming { .. } => AuthorizationView::Arming,\n                    Authorization::Armed { .. } => AuthorizationView::Armed,\n                },\n                audit_health: match &active.audit_health {\n",
        "                authorization: match &active.authorization {\n                    Authorization::ProcessDisabled => AuthorizationView::ProcessDisabled,\n                    Authorization::Disarmed { .. } => AuthorizationView::Disarmed,\n                    Authorization::Arming { .. } => AuthorizationView::Arming,\n                    Authorization::Armed { .. } => AuthorizationView::Armed,\n                },\n                arming_challenge: match &active.authorization {\n                    Authorization::Arming { challenge, .. } => Some(challenge.clone()),\n                    _ => None,\n                },\n                arming_expires_at: match &active.authorization {\n                    Authorization::Arming { expires_at, .. } => Some(*expires_at),\n                    _ => None,\n                },\n                armed_idle_expires_at: match &active.authorization {\n                    Authorization::Armed { idle_expires_at } => Some(*idle_expires_at),\n                    _ => None,\n                },\n                audit_health: match &active.audit_health {\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "            authorization: AuthorizationView::Unavailable,\n            audit_health: AuditHealthView::Unavailable,\n",
        "            authorization: AuthorizationView::Unavailable,\n            arming_challenge: None,\n            arming_expires_at: None,\n            armed_idle_expires_at: None,\n            audit_health: AuditHealthView::Unavailable,\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "    pub const fn authorization(&self) -> AuthorizationView {\n        self.authorization\n    }\n\n    #[must_use]\n    pub const fn audit_health(&self) -> AuditHealthView {\n",
        "    pub const fn authorization(&self) -> AuthorizationView {\n        self.authorization\n    }\n\n    #[must_use]\n    pub fn arming_challenge(&self) -> Option<&str> {\n        self.arming_challenge.as_deref()\n    }\n\n    #[must_use]\n    pub const fn arming_expires_at(&self) -> Option<Instant> {\n        self.arming_expires_at\n    }\n\n    #[must_use]\n    pub const fn armed_idle_expires_at(&self) -> Option<Instant> {\n        self.armed_idle_expires_at\n    }\n\n    #[must_use]\n    pub const fn audit_health(&self) -> AuditHealthView {\n",
    );

    // Project the prepared plan into an immutable presentation value. It cannot execute a write;
    // the coordinator-owned PlanId entry is still required and consumed by confirm_write.
    replace_once(
        "crates/lantern-app/src/parameters.rs",
        "    pub staged_intent: Option<StagedWriteIntent>,\n    pub error: Option<String>,\n",
        "    pub staged_intent: Option<StagedWriteIntent>,\n    pub prepared_write: Option<PreparedWritePlan>,\n    pub write_status: Option<String>,\n    pub error: Option<String>,\n",
    );
    replace_once(
        "crates/lantern-app/src/parameters.rs",
        "    staged_intent: Option<StagedWriteIntent>,\n    error: Option<&str>,\n) -> ParameterBrowserView {\n",
        "    staged_intent: Option<StagedWriteIntent>,\n    prepared_write: Option<PreparedWritePlan>,\n    write_status: Option<String>,\n    error: Option<&str>,\n) -> ParameterBrowserView {\n",
    );
    replace_once(
        "crates/lantern-app/src/parameters.rs",
        "        latest,\n        staged_intent,\n        error: error.map(str::to_owned),\n",
        "        latest,\n        staged_intent,\n        prepared_write,\n        write_status,\n        error: error.map(str::to_owned),\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "                        self.parameters.staged_intent.clone(),\n                        self.parameters.error.as_deref(),\n",
        "                        self.parameters.staged_intent.clone(),\n                        self.parameters.prepared_write.clone(),\n                        self.parameters.write_status.clone(),\n                        self.parameters.error.as_deref(),\n",
    );

    // Presentation-owned text entry modes for arming and phase-2 confirmation.
    replace_once(
        "crates/lantern-tui/src/ui_state.rs",
        "    ParameterSearch,\n}\n",
        "    ParameterSearch,\n    WriteArming,\n    WriteConfirmation,\n}\n",
    );
    replace_once(
        "crates/lantern-tui/src/ui_state.rs",
        "    ClearParameterSearch,\n    SetParameterGroup(Option<String>),\n",
        "    ClearParameterSearch,\n    BeginWriteArming,\n    BeginWriteConfirmation,\n    SetParameterGroup(Option<String>),\n",
    );
    replace_once(
        "crates/lantern-tui/src/ui_state.rs",
        "            UiAction::ClearParameterSearch => {\n                self.parameters.filters.search.clear();\n                self.form.clear();\n                self.connection_edit = None;\n                self.selected_index = 0;\n                self.focus = Focus::Navigation;\n            }\n            UiAction::SetParameterGroup(value) => {\n",
        "            UiAction::ClearParameterSearch => {\n                self.parameters.filters.search.clear();\n                self.form.clear();\n                self.connection_edit = None;\n                self.selected_index = 0;\n                self.focus = Focus::Navigation;\n            }\n            UiAction::BeginWriteArming => {\n                self.form.clear();\n                self.connection_edit = Some(ConnectionEdit::WriteArming);\n                self.parameters.editor = None;\n                self.focus = Focus::Content;\n            }\n            UiAction::BeginWriteConfirmation => {\n                self.form.clear();\n                self.connection_edit = Some(ConnectionEdit::WriteConfirmation);\n                self.parameters.editor = None;\n                self.focus = Focus::Content;\n            }\n            UiAction::SetParameterGroup(value) => {\n",
    );

    // Global edit handling must capture q and all other characters as text while the safety
    // confirmation form owns focus.
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "use std::{collections::BTreeSet, path::PathBuf};\n",
        "use std::{\n    collections::BTreeSet,\n    path::PathBuf,\n    time::{Duration, Instant},\n};\n",
    );
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "    MonitoringAction, ScopePanel, SessionInput,\n",
        "    MonitoringAction, ParameterAction, ScopePanel, SessionInput,\n",
    );
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "pub const HELP_BINDINGS: [KeyBinding; 45] = [\n",
        "pub const HELP_BINDINGS: [KeyBinding; 47] = [\n",
    );
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "    KeyBinding {\n        key: \"Parameters e\",\n        description: \"open typed WriteIntent preview editor; never write\",\n    },\n    KeyBinding {\n        key: \"Parameters c\",\n        description: \"clear staged WriteIntent preview\",\n    },\n",
        "    KeyBinding {\n        key: \"Parameters e\",\n        description: \"stage a typed WriteIntent from a fresh Good value\",\n    },\n    KeyBinding {\n        key: \"Parameters A\",\n        description: \"start/confirm arming challenge or disarm writes\",\n    },\n    KeyBinding {\n        key: \"Parameters w\",\n        description: \"prepare guarded plan, then open exact confirmation\",\n    },\n    KeyBinding {\n        key: \"Parameters c\",\n        description: \"cancel staged/prepared guarded write\",\n    },\n",
    );
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "            ConnectionEdit::ParameterSearch => match key.code {\n                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),\n                KeyCode::Enter => Some(MappedAction::Ui(UiAction::ApplyParameterSearch)),\n                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),\n                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),\n                _ => None,\n            },\n",
        "            ConnectionEdit::ParameterSearch => match key.code {\n                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),\n                KeyCode::Enter => Some(MappedAction::Ui(UiAction::ApplyParameterSearch)),\n                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),\n                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),\n                _ => None,\n            },\n            ConnectionEdit::WriteArming => match key.code {\n                KeyCode::Esc => Some(MappedAction::Combined {\n                    ui: UiAction::CancelEdit,\n                    application: Box::new(ApplicationAction::Session(SessionInput::CancelArming)),\n                }),\n                KeyCode::Enter => {\n                    let now = Instant::now();\n                    let idle_expires_at = now\n                        .checked_add(Duration::from_secs(60))\n                        .unwrap_or(now);\n                    Some(MappedAction::Combined {\n                        ui: UiAction::CancelEdit,\n                        application: Box::new(ApplicationAction::Session(\n                            SessionInput::ConfirmArming {\n                                challenge: ui.form.value().to_owned(),\n                                now,\n                                idle_expires_at,\n                            },\n                        )),\n                    })\n                }\n                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),\n                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),\n                _ => None,\n            },\n            ConnectionEdit::WriteConfirmation => match key.code {\n                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),\n                KeyCode::Enter => Some(MappedAction::Combined {\n                    ui: UiAction::CancelEdit,\n                    application: Box::new(ApplicationAction::Parameters(\n                        ParameterAction::ConfirmPrepared {\n                            operator_text: ui.form.value().to_owned(),\n                        },\n                    )),\n                }),\n                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),\n                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),\n                _ => None,\n            },\n",
    );

    // Parameter-screen safety controls. Staging an intent is not arming and not preparation.
    replace_once(
        "crates/lantern-tui/src/parameter_keymap.rs",
        "use crossterm::event::{KeyCode, KeyEvent};\n",
        "use std::time::{Duration, Instant};\n\nuse crossterm::event::{KeyCode, KeyEvent};\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_keymap.rs",
        "    ApplicationAction, ApplicationView, AuthorizationView, EngineeringValue, ParameterAccess,\n    ParameterAction, ParameterEditorInput, ParameterEditorKind, ParameterRiskView, QuantityKind,\n    TelemetryQuality,\n",
        "    ApplicationAction, ApplicationView, AuditHealthView, AuthorizationView, EngineeringValue,\n    OperationView, ParameterAccess, ParameterAction, ParameterEditorInput, ParameterEditorKind,\n    ParameterRiskView, QuantityKind, SessionInput, SessionPhaseView, TelemetryQuality,\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_keymap.rs",
        "        KeyCode::Char('e') => begin_editor_action(ui, view),\n        KeyCode::Char('c') if browser.staged_intent.is_some() => {\n            Some(parameter_action(ParameterAction::ClearIntent))\n        }\n",
        "        KeyCode::Char('e') => begin_editor_action(ui, view),\n        KeyCode::Char('A') => arm_action(view),\n        KeyCode::Char('w') => guarded_write_action(view),\n        KeyCode::Char('c')\n            if browser.staged_intent.is_some() || browser.prepared_write.is_some() =>\n        {\n            Some(parameter_action(ParameterAction::ClearIntent))\n        }\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_keymap.rs",
        "fn begin_editor_action(ui: &UiState, view: &ApplicationView) -> Option<MappedAction> {\n",
        r#"fn arm_action(view: &ApplicationView) -> Option<MappedAction> {
    let session = view.session();
    match session.authorization() {
        AuthorizationView::ProcessDisabled => Some(MappedAction::Ui(UiAction::ShowMessage {
            title: "Write arming unavailable".to_owned(),
            body: "Restart with --enable-writes to make explicit arming available. Read-only remains the default."
                .to_owned(),
        })),
        AuthorizationView::Disarmed => {
            if session.phase() != SessionPhaseView::Connected
                || session.audit_health() != AuditHealthView::Healthy
                || session.operation() != OperationView::Idle
            {
                return Some(MappedAction::Ui(UiAction::ShowMessage {
                    title: "Cannot arm writes".to_owned(),
                    body: "Arming requires Connected + Verified + audit Healthy + operation Idle."
                        .to_owned(),
                }));
            }
            let profile_hash = session.profile_hash()?;
            let prefix_len = profile_hash.len().min(12);
            let challenge = format!("ARM {}", &profile_hash[..prefix_len]);
            let now = Instant::now();
            let expires_at = now.checked_add(Duration::from_secs(30)).unwrap_or(now);
            Some(MappedAction::Combined {
                ui: UiAction::BeginWriteArming,
                application: Box::new(ApplicationAction::Session(SessionInput::ArmWrites {
                    challenge,
                    expires_at,
                })),
            })
        }
        AuthorizationView::Arming => Some(MappedAction::Ui(UiAction::BeginWriteArming)),
        AuthorizationView::Armed => Some(MappedAction::Application(Box::new(
            ApplicationAction::Session(SessionInput::DisarmWrites),
        ))),
        AuthorizationView::Unavailable => None,
    }
}

fn guarded_write_action(view: &ApplicationView) -> Option<MappedAction> {
    let session = view.session();
    if session.authorization() != AuthorizationView::Armed {
        return Some(MappedAction::Ui(UiAction::ShowMessage {
            title: "Writes are disarmed".to_owned(),
            body: "Press A and complete the exact arming challenge before preparing a guarded write."
                .to_owned(),
        }));
    }
    if session.phase() != SessionPhaseView::Connected
        || session.audit_health() != AuditHealthView::Healthy
        || session.operation() != OperationView::Idle
    {
        return Some(MappedAction::Ui(UiAction::ShowMessage {
            title: "Guarded write blocked".to_owned(),
            body: "Write preparation requires Connected + Verified + Armed + audit Healthy + operation Idle."
                .to_owned(),
        }));
    }
    let browser = view.parameters();
    if browser.prepared_write.is_some() {
        return Some(MappedAction::Ui(UiAction::BeginWriteConfirmation));
    }
    if browser.staged_intent.is_some() {
        return Some(parameter_action(ParameterAction::PrepareWrite));
    }
    Some(MappedAction::Ui(UiAction::ShowMessage {
        title: "No staged WriteIntent".to_owned(),
        body: "Select a writable parameter, press e, and stage a typed intent from a fresh Good observation first."
            .to_owned(),
    }))
}

fn begin_editor_action(ui: &UiState, view: &ApplicationView) -> Option<MappedAction> {
"#,
    );
    replace_once(
        "crates/lantern-tui/src/parameter_keymap.rs",
        "            body: \"Restart with --enable-writes to prepare a WriteIntent. This screen never executes a write.\"\n                .to_owned(),\n",
        "            body: \"Restart with --enable-writes to stage a WriteIntent. Execution still requires Verified + trust + audit Healthy + explicit arming + prepare/confirm.\"\n                .to_owned(),\n",
    );

    // Render every safety stage explicitly; nothing calls transport from presentation code.
    replace_once(
        "crates/lantern-tui/src/screens.rs",
        "        Screen::Parameters => parameter_lines(\n            view.parameters(),\n            view.active_session().is_some(),\n            view.session().authorization(),\n            ui,\n        ),\n",
        "        Screen::Parameters => parameter_lines(\n            view.parameters(),\n            view.active_session().is_some(),\n            view.session(),\n            ui,\n        ),\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "    AuthorizationView, EngineeringValue, LatestValue, ParameterBrowserView,\n    ParameterDescriptorView, ParameterEditorKind, RawRegisters, TelemetryQuality,\n",
        "    AuthorizationView, EngineeringValue, LatestValue, ParameterBrowserView,\n    ParameterDescriptorView, ParameterEditorKind, RawRegisters, SessionView, TelemetryQuality,\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "    ParameterEditorUiState, UiState, filtered_parameters, selected_parameter, visible_parameter_ids,\n",
        "    ConnectionEdit, ParameterEditorUiState, UiState, filtered_parameters, selected_parameter,\n    visible_parameter_ids,\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "    authorization: AuthorizationView,\n    ui: &UiState,\n) -> Vec<Line<'static>> {\n",
        "    session: &SessionView,\n    ui: &UiState,\n) -> Vec<Line<'static>> {\n    let authorization = session.authorization();\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "        \"/ search | x clear search | g group | a access | y quality | u unreadable | r risk | t quantity | R refresh | e prepare intent | c clear preview\",\n",
        "        \"/ search | filters g/a/y/u/r/t | R refresh | e stage intent | A arm/disarm | w prepare/confirm | c cancel\",\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "    if let Some(error) = &browser.error {\n        lines.push(Line::from(format!(\"PARAMETER ERROR: {error}\")));\n    }\n    lines.push(Line::from(\"\"));\n",
        r#"    if let Some(error) = &browser.error {
        lines.push(Line::from(format!("PARAMETER ERROR: {error}")));
    }
    if let Some(status) = &browser.write_status {
        lines.push(Line::from(format!("WRITE STATUS: {status}")));
    }
    match authorization {
        AuthorizationView::Arming => {
            lines.push(Line::from(format!(
                "ARMING CHALLENGE: {}",
                session.arming_challenge().unwrap_or("unavailable")
            )));
            lines.push(Line::from(
                "Type the challenge exactly and press Enter; Esc cancels arming. Challenge expires after 30 seconds.",
            ));
            if ui.connection_edit == Some(ConnectionEdit::WriteArming) {
                lines.push(Line::from(format!("Arming confirmation: {}_", ui.form.value())));
            }
        }
        AuthorizationView::Armed => lines.push(Line::from(
            "WRITES ARMED — only guarded prepare/confirm may reach the single-write capability; A disarms.",
        )),
        AuthorizationView::Disarmed => lines.push(Line::from(
            "WRITES DISARMED — press A to start an explicit short-lived arming challenge.",
        )),
        AuthorizationView::ProcessDisabled => lines.push(Line::from(
            "READ-ONLY PROCESS — restart with --enable-writes before arming can exist.",
        )),
        AuthorizationView::Unavailable => {}
    }
    lines.push(Line::from(""));
"#,
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "        lines.push(Line::from(\"WRITE INTENT PREVIEW — NO WRITE SENT\"));\n",
        "        lines.push(Line::from(\"STAGED WRITE INTENT — NO WRITE SENT\"));\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "        lines.push(Line::from(\n            \"Policy, target raw, write function and read-back remain authoritative in the active profile/#16; this preview cannot execute Modbus write I/O.\",\n        ));\n    }\n    lines\n}\n",
        r#"        lines.push(Line::from(
            "This is only an intent. Press w while Armed to ask WriteCoordinator for a fresh guarded plan; no Modbus write has been sent.",
        ));
    }
    if let Some(plan) = &browser.prepared_write {
        lines.push(Line::from(""));
        lines.push(Line::from("PREPARED GUARDED WRITE — NO WRITE SENT YET"));
        lines.push(Line::from(format!(
            "parameter={} previous_raw={} requested={} target_raw={}",
            plan.parameter_id(),
            raw_label(plan.previous_raw()),
            engineering_label(plan.requested_engineering()),
            raw_label(plan.target_raw()),
        )));
        lines.push(Line::from(format!(
            "challenge={} exact-confirmation={:?}",
            plan.challenge(),
            plan.operator_confirmation_text(),
        )));
        lines.push(Line::from(
            "Press w to open phase-2 confirmation. Only an exact match can call confirm_write; Esc leaves the plan prepared, c cancels it.",
        ));
        if ui.connection_edit == Some(ConnectionEdit::WriteConfirmation) {
            lines.push(Line::from(format!("Write confirmation: {}_", ui.form.value())));
        }
    }
    lines
}
"#,
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "                \"Editor gated: process writes are disabled. Use --enable-writes only to prepare an intent; no write is executed here.\"\n                    .to_owned()\n",
        "                \"Editor gated: process writes are disabled. --enable-writes only makes explicit arming and the guarded pipeline available.\"\n                    .to_owned()\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "                \"Enter validates engineering→raw preview; Esc cancels. No write request is created.\",\n",
        "                \"Enter validates engineering→raw and stages an intent; Esc cancels. No write request is created yet.\",\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "                    \"j/k selects only a profile-declared enum value; Enter prepares preview; Esc cancels.\",\n",
        "                    \"j/k selects only a profile-declared enum value; Enter stages intent; Esc cancels.\",\n",
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "                    \"j/k selects a declared flag; Space toggles it; Enter prepares preview; Esc cancels.\",\n",
        "                    \"j/k selects a declared flag; Space toggles it; Enter stages intent; Esc cancels.\",\n",
    );

    // Drive arming and idle expiry using std::Instant values owned by SessionStateMachine state.
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "    let mut sigint = signal(SignalKind::interrupt())?;\n    let mut sigterm = signal(SignalKind::terminate())?;\n",
        "    let mut sigint = signal(SignalKind::interrupt())?;\n    let mut sigterm = signal(SignalKind::terminate())?;\n    let mut write_safety_tick = tokio::time::interval(Duration::from_millis(250));\n    write_safety_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);\n",
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "            event = next_port_event(&mut port_events) => {\n                if let Some(event) = event {\n                    application.dispatch(ApplicationAction::Connection(ConnectionAction::PortEvent(event)))?;\n                    dirty = true;\n                }\n            }\n            _ = sigint.recv() => {\n",
        "            event = next_port_event(&mut port_events) => {\n                if let Some(event) = event {\n                    application.dispatch(ApplicationAction::Connection(ConnectionAction::PortEvent(event)))?;\n                    dirty = true;\n                }\n            }\n            _ = write_safety_tick.tick() => {\n                let session = application.state().view().session().clone();\n                let now = Instant::now();\n                if session.arming_expires_at().is_some_and(|deadline| now >= deadline) {\n                    application.dispatch(ApplicationAction::Session(SessionInput::ArmingExpired))?;\n                    dirty = true;\n                } else if session\n                    .armed_idle_expires_at()\n                    .is_some_and(|deadline| now >= deadline)\n                {\n                    application.dispatch(ApplicationAction::Session(SessionInput::IdleDisarmElapsed))?;\n                    dirty = true;\n                }\n            }\n            _ = sigint.recv() => {\n",
    );

    // Architecture acceptance: the user-facing write flow must cross SessionStateMachine and
    // ParameterAction boundaries, never a direct transport write.
    replace_once(
        "scripts/check-architecture.sh",
        "printf 'architecture checks passed\\n'\n",
        "if ! grep -q 'SessionInput::ArmWrites' crates/lantern-tui/src/parameter_keymap.rs \\\n    || ! grep -q 'ParameterAction::PrepareWrite' crates/lantern-tui/src/parameter_keymap.rs \\\n    || ! grep -q 'ParameterAction::ConfirmPrepared' crates/lantern-tui/src/keymap.rs; then\n    printf 'issue #23 requires explicit arming, prepare and phase-2 confirmation in the TUI boundary\\n' >&2\n    exit 1\nfi\n\nif [ ! -f docs/development/threat-model.md ]; then\n    printf 'issue #23 requires an explicit industrial threat model\\n' >&2\n    exit 1\nfi\n\nprintf 'architecture checks passed\\n'\n",
    );

    write(
        "docs/development/threat-model.md",
        r#"# Threat model and safety boundary

VFD Lantern is **not safety-rated**. It does not replace an E-stop, hardware interlocks, LOTO,
manufacturer procedures, or qualified personnel. The application defaults to read-only. A write
can become reachable only after Verified identification, trusted exact profile hash, healthy durable
audit, explicit short-lived arming, two-phase prepare/confirm, and the coordinator-owned single-write
capability.

## Trust boundaries

| Threat / actor | Boundary and invariant | Verification | Owner |
| --- | --- | --- | --- |
| Accidental corruption or stale local files | Parsers are bounded and fail closed; profile semantic hash binds trust and approvals; symlinks and irregular sensitive files are rejected; durable audit is append/finalize verified. | profile validation/hash tests, storage symlink/atomic/audit verifier tests | profile + storage |
| Malicious or malfunctioning VFD | A response is not identity. Only bounded read-only probes create a Verified session. Writes use profile-declared addresses/functions, fresh old value, fresh authoritative drive state, one physical write, bounded read-back, no write retry and no rollback. | identification mismatch/timeout tests, simulator wire-fault tests, WriteCoordinator E2E | app + transport |
| Another process running as the same user | Serial open/exclusivity is best-effort kernel protection; every guarded write revalidates fresh device state immediately before the single write. No claim is made that a hostile peer process can be cryptographically excluded from the serial device. | serial open/exclusivity tests and precondition-change E2E | transport + app |
| Account owner | The account owner can alter its local trust store and user-owned files. Local approval is therefore an explicit operator decision bound to an exact profile hash, **not** a cryptographic root of trust. Packaged origin never comes from that store. | RuntimeProfileTrust exact-hash/corruption tests | storage |
| root / installation owner | An owner able to replace the executable, system profiles, runner, or installation can replace the whole trust boundary. This is explicitly outside the runtime guarantee. Packaged trust only proves agreement with the manifest embedded in the currently running binary. | embedded-manifest/package-copy tests | packaging + release |

## Production write invariant

The composition root owns the only production path to `WriteCoordinator`. It supplies
`FilesystemAuditPort`, `RuntimeProfileTrust`, the current `BusActorHandle`, a monotonic clock and a
`SessionControlPort`. If durable audit is unavailable, no coordinator is constructed. An untrusted
profile is rejected by `ProfileTrustPort`. Presentation code can only emit application actions; it
cannot obtain `PreparedBusWrite` or call transport write methods.

The operator sequence is deliberately non-atomic from a UI perspective:

1. start the process with `--enable-writes` (otherwise `ProcessDisabled`),
2. complete a short-lived exact arming challenge,
3. stage a typed `WriteIntent` from a fresh Good observation,
4. ask `WriteCoordinator` to prepare a fresh plan,
5. inspect old/target/challenge and type the exact phase-2 confirmation,
6. coordinator revalidates session, trust, fresh old value and authoritative drive state,
7. durable audit prepare succeeds,
8. exactly one physical write is attempted, followed by bounded read-back and audit finalize.

Reconnect, identity mismatch, unknown write outcome, audit degradation, arming expiry and armed-idle
expiry all remove write authorization. There is no automatic write, automatic restore, write retry,
rollback, raw-PDU escape hatch, broadcast slave, motion command, or fault-reset path.

## Files, privacy and network

Sensitive state belongs to storage adapters using private directories/files and no-follow/atomic
patterns. Diagnostic logs do not contain raw frames or telemetry values by default. Values, CSV,
backup, audit, full profile and fault payload inclusion requires explicit opt-in where diagnostics
can contain them. The application has no update service, telemetry service, or server requirement.

## Residual risks

This model cannot protect against compromised firmware, kernel, runtime account, root, physical bus
injection, or replacement of the running binary by the installation owner. Those risks require
operational controls outside VFD Lantern: physical isolation, access control, LOTO, hardware safety
circuits, signed/reviewed distribution and qualified commissioning procedures.
"#,
    );
}
