//! In-TUI provider credential manager: browse the model catalog's providers
//! and named accounts, add/replace/remove a key, and pick a default model --
//! all without leaving the app.
//!
//! `bcode_shell::auth::provider_setup` owns the credential policy (validation,
//! storage, verification); this view is a pure front end for it. `handle_key`
//! does no I/O -- it returns a [`ProviderManagerOutcome`] the host turns into
//! an `Action`/`Effect`, exactly like every other modal in this crate.
//!
//! The same state renders full-screen as the first-run experience (no
//! credential resolves anywhere yet, so bcode has nothing else to show) and
//! inside [`crate::views::modal_window`] mid-session (`/providers`, `/login`).

use crate::input::line_editor::{LineEditOutcome, LineEditor};
use crate::theme::Theme;
use crate::views::masked_input::render_masked_input_box;
use crate::views::modal_window::{
    ModalContentArea, ModalSizing, ModalWindowConfig, ModalWindowState, Shortcut,
    render_modal_window,
};
use bcode_shell::auth::accounts::AccountKind;
use bcode_shell::auth::provider_setup::{self, AccountStatus, CredentialSource, ProviderStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget};
use std::path::Path;

/// Which list is showing: catalog providers, or named `[accounts.*]` entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTab {
    Providers,
    Accounts,
}

/// What a key-entry / remove-confirm action applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyTarget {
    /// A catalog provider, by its `id` (`provider::<id>` in `auth.json`).
    Provider(String),
    /// A named account (`account::<name>` in `auth.json`).
    Account(String),
}

/// Sub-mode of the manager. `Browse` is the resting state; everything else is
/// a short-lived interaction layered on top of it.
#[derive(Debug)]
pub enum ProviderMode {
    Browse,
    /// Typing a key for `target`. `error` shows a view-side rejection (e.g.
    /// blank key) that never reached disk.
    EnteringKey {
        target: KeyTarget,
        editor: LineEditor,
        error: Option<String>,
    },
    /// The host is verifying a just-submitted key against the provider's API.
    /// Purely a rendering state; the host clears it via [`Self::finish_key_verification`].
    Verifying {
        target: KeyTarget,
    },
    /// A browser-based OAuth sign-in is running for this provider. The host
    /// runs the shell's provider login flow; completion reloads statuses.
    SigningIn {
        provider_id: String,
        provider_name: String,
        auth_url: Option<String>,
        editor: LineEditor,
    },
    /// Naming a new account before its key-entry step.
    NamingAccount {
        editor: LineEditor,
        error: Option<String>,
    },
    /// `d` was pressed on a row with a stored credential; `y`/Enter confirms.
    ConfirmRemove {
        target: KeyTarget,
    },
    /// A provider key was just stored and this is the first credential to
    /// resolve in an auth-gated manager (first run, or a blocked
    /// `/login`/401 prompt): offer to make its catalog default the session
    /// default before finishing authentication.
    OfferDefaultModel {
        provider_id: String,
        model: String,
    },
}

/// A pure outcome of a key event. `Unchanged`/`Changed` are redraw hints with
/// no side effect; everything else is something for the host to turn into an
/// `Action`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderManagerOutcome {
    Unchanged,
    Changed,
    /// Close the manager (mid-session only -- first-run has nowhere to go).
    Close,
    /// Persist `key` for `target`. The host stores it, then (for a provider
    /// target) verifies it and calls [`ProviderManagerState::finish_key_verification`].
    StoreKey {
        target: KeyTarget,
        key: String,
    },
    /// Remove the stored credential for `target`.
    RemoveKey {
        target: KeyTarget,
    },
    /// Set this catalog model id as `models.default`.
    SetDefaultModel(String),
    /// Start a browser-based OAuth sign-in for this provider (it has an
    /// `auth` entry in the catalog, e.g. a ChatGPT subscription).
    OAuthLogin {
        provider_id: String,
    },
    /// The user declined the post-setup default-model offer. Purely a signal
    /// for the host to finish authentication anyway (the credential still
    /// resolves; the user just didn't want to change the default).
    DismissDefaultOffer,
    /// First-run only: leave the app (the Quit row, or `q`).
    Quit,
    /// Abort an in-flight browser OAuth sign-in and return to Browse.
    CancelOAuth,
    /// User pasted or typed a callback URL / auth code during OAuth.
    SubmitOAuthCode(String),
}

const MAX_KEY_BYTES: usize = 4000;
const MAX_ACCOUNT_NAME_BYTES: usize = 64;

/// State for the provider manager view. Holds its own snapshot of provider /
/// account status, refreshed by [`Self::reload`].
pub struct ProviderManagerState {
    pub window: ModalWindowState,
    pub tab: ProviderTab,
    providers: Vec<ProviderStatus>,
    accounts: Vec<AccountStatus>,
    pub selected: usize,
    pub mode: ProviderMode,
    /// `(message, is_error)` shown under the list until the next state change.
    pub notice: Option<(String, bool)>,
    /// `true` when this is the first-run, full-screen experience (no
    /// credential resolves anywhere, and no session or menu exists yet):
    /// closing is not offered, because there is nothing else to show.
    pub first_run: bool,
    /// `true` when the manager is standing in for a blocked login (first run,
    /// or `/login`/a 401 prompt finding nothing to sign in to): storing a
    /// credential that now resolves should finish authentication and start
    /// or resume a session. `false` for the always-available `/providers`
    /// management surface, where a session already works and adding another
    /// provider is just bookkeeping.
    pub auth_gated: bool,
}

impl ProviderManagerState {
    fn build(home: &Path, first_run: bool, auth_gated: bool) -> Self {
        Self {
            window: ModalWindowState::with_tabs(2),
            tab: ProviderTab::Providers,
            providers: provider_setup::provider_status(home),
            accounts: provider_setup::account_status_from_effective_config(home),
            selected: 0,
            mode: ProviderMode::Browse,
            notice: None,
            first_run,
            auth_gated,
        }
    }

    /// First-run, full-screen construction: bcode has no credential anywhere
    /// and no session exists yet.
    pub fn first_run(home: &Path) -> Self {
        Self::build(home, true, true)
    }

    /// Mid-session construction for a blocked `/login`/401 prompt: nothing
    /// resolves anywhere, but (unlike first-run) there is a previous view to
    /// restore to on close.
    pub fn reauth(home: &Path) -> Self {
        Self::build(home, false, true)
    }

    /// Mid-session, always-available management construction (`/providers`).
    /// A credential already resolves; storing another is not an auth gate.
    pub fn open(home: &Path) -> Self {
        Self::build(home, false, false)
    }

    /// Re-read provider/account status from disk. Call after a store/remove
    /// completes so the list reflects what actually landed.
    pub fn reload(&mut self, home: &Path) {
        self.providers = provider_setup::provider_status(home);
        self.accounts = provider_setup::account_status_from_effective_config(home);
        let len = self.row_count();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    /// Whether any provider or account currently resolves a credential.
    /// The host uses this after a successful store to decide whether a
    /// first-run manager is [`ProviderManagerOutcome::Ready`].
    pub fn has_any_usable_credential(&self) -> bool {
        self.providers.iter().any(|p| p.source.is_usable())
            || self.accounts.iter().any(|a| a.has_stored_key)
    }

    pub(crate) fn list_len(&self) -> usize {
        match self.tab {
            ProviderTab::Providers => self.providers.len(),
            ProviderTab::Accounts => self.accounts.len(),
        }
    }

    fn row_count(&self) -> usize {
        let n = self.list_len();
        if self.first_run && matches!(self.mode, ProviderMode::Browse) {
            n + 1
        } else {
            n
        }
    }

    fn is_quit_row(&self) -> bool {
        self.first_run
            && matches!(self.mode, ProviderMode::Browse)
            && self.selected == self.list_len()
    }

    /// Welcome-menu rows for the first-run stacked layout: `(status, name)`,
    /// plus a trailing Quit row so the screen is clickable like every other
    /// blocked welcome state.
    pub fn browse_menu_items(&self) -> Vec<(String, String)> {
        let mut items: Vec<(String, String)> = match self.tab {
            ProviderTab::Providers => self
                .providers
                .iter()
                .map(|row| (provider_row_status(row), row.info.name.clone()))
                .collect(),
            ProviderTab::Accounts => self
                .accounts
                .iter()
                .map(|row| (account_row_status(row), row.name.clone()))
                .collect(),
        };
        if self.first_run {
            items.push(("ctrl+q".to_string(), "Quit".to_string()));
        }
        items
    }

    fn selected_provider(&self) -> Option<&ProviderStatus> {
        match self.tab {
            ProviderTab::Providers => self.providers.get(self.selected),
            ProviderTab::Accounts => None,
        }
    }

    fn selected_account(&self) -> Option<&AccountStatus> {
        match self.tab {
            ProviderTab::Accounts => self.accounts.get(self.selected),
            ProviderTab::Providers => None,
        }
    }

    /// The host calls this once a store (and, for a provider target, verify)
    /// round trip for `target` completes. `verify_result` is `None` for an
    /// account target (never verified) or a store that failed before
    /// verification ran. Clears `Verifying` back to `Browse`, but only if the
    /// manager is still showing that same target -- the user may have moved
    /// on while the network call was in flight.
    pub fn finish_store(
        &mut self,
        home: &Path,
        target: &KeyTarget,
        verify_result: Option<Result<(), String>>,
    ) {
        self.reload(home);
        if matches!(&self.mode, ProviderMode::Verifying { target: t } if t == target) {
            self.mode = match target {
                KeyTarget::Provider(id)
                    if self.auth_gated
                        && self
                            .providers
                            .iter()
                            .find(|p| &p.info.id == id)
                            .is_some_and(|p| p.source.is_usable()) =>
                {
                    ProviderMode::OfferDefaultModel {
                        provider_id: id.clone(),
                        model: bcode_models::provider(id)
                            .map(|p| p.default_model.clone())
                            .unwrap_or_default(),
                    }
                }
                _ => ProviderMode::Browse,
            };
        }
        self.notice = Some(match verify_result {
            None => (format!("{} saved", target_label(target)), false),
            Some(Ok(())) => (format!("{} verified", target_label(target)), false),
            Some(Err(e)) => (
                format!(
                    "{} saved, but verification failed: {e}",
                    target_label(target)
                ),
                true,
            ),
        });
    }

    /// The host calls this once a credential removal for `target` completes.
    pub fn finish_remove(&mut self, home: &Path, target: &KeyTarget, result: Result<bool, String>) {
        self.reload(home);
        self.notice = Some(match result {
            Ok(true) => (format!("{} removed", target_label(target)), false),
            Ok(false) => (format!("{} had no stored key", target_label(target)), false),
            Err(e) => (
                format!("failed to remove {}: {e}", target_label(target)),
                true,
            ),
        });
    }

    /// The host calls this once a browser-based OAuth sign-in for `provider_id`
    /// finishes. Reloads statuses and, on success, leaves `SigningIn` for the
    /// host to decide whether authentication is now satisfied.
    pub fn finish_oauth(&mut self, home: &Path, provider_id: &str, result: Result<(), String>) {
        self.reload(home);
        if matches!(&self.mode, ProviderMode::SigningIn { provider_id: id, .. } if id == provider_id)
        {
            self.mode = if self.auth_gated
                && result.is_ok()
                && self
                    .providers
                    .iter()
                    .find(|p| p.info.id == provider_id)
                    .is_some_and(|p| p.source.is_usable())
            {
                ProviderMode::OfferDefaultModel {
                    provider_id: provider_id.to_string(),
                    model: bcode_models::provider(provider_id)
                        .map(|p| p.default_model.clone())
                        .unwrap_or_default(),
                }
            } else {
                ProviderMode::Browse
            };
        }
        self.notice = Some(match result {
            Ok(()) => (format!("{provider_id}: signed in"), false),
            Err(e) => (format!("{provider_id}: sign-in failed: {e}"), true),
        });
    }

    pub fn set_oauth_url(&mut self, provider_id: &str, url: String) {
        if let ProviderMode::SigningIn {
            provider_id: id,
            auth_url,
            ..
        } = &mut self.mode
            && id == provider_id
        {
            *auth_url = Some(url);
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> ProviderManagerOutcome {
        match &mut self.mode {
            ProviderMode::Browse => self.handle_browse_key(key),
            ProviderMode::EnteringKey { .. } => self.handle_entering_key(key),
            ProviderMode::Verifying { .. } => ProviderManagerOutcome::Unchanged,
            ProviderMode::SigningIn { .. } => self.handle_signing_in_key(key),
            ProviderMode::NamingAccount { .. } => self.handle_naming_account_key(key),
            ProviderMode::ConfirmRemove { target } => {
                let target = target.clone();
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                        self.mode = ProviderMode::Browse;
                        ProviderManagerOutcome::RemoveKey { target }
                    }
                    _ => {
                        self.mode = ProviderMode::Browse;
                        ProviderManagerOutcome::Changed
                    }
                }
            }
            ProviderMode::OfferDefaultModel { model, .. } => {
                let model = model.clone();
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                        self.mode = ProviderMode::Browse;
                        ProviderManagerOutcome::SetDefaultModel(model)
                    }
                    _ => {
                        self.mode = ProviderMode::Browse;
                        ProviderManagerOutcome::DismissDefaultOffer
                    }
                }
            }
        }
    }

    fn handle_browse_key(&mut self, key: &KeyEvent) -> ProviderManagerOutcome {
        let len = self.row_count();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if len > 0 && self.selected > 0 {
                    self.selected -= 1;
                    return ProviderManagerOutcome::Changed;
                }
                ProviderManagerOutcome::Unchanged
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 0 && self.selected + 1 < len {
                    self.selected += 1;
                    return ProviderManagerOutcome::Changed;
                }
                ProviderManagerOutcome::Unchanged
            }
            KeyCode::Tab | KeyCode::BackTab => self.set_tab(match self.tab {
                ProviderTab::Providers => ProviderTab::Accounts,
                ProviderTab::Accounts => ProviderTab::Providers,
            }),
            KeyCode::Char('a') if self.tab == ProviderTab::Accounts => {
                self.mode = ProviderMode::NamingAccount {
                    editor: LineEditor::default(),
                    error: None,
                };
                ProviderManagerOutcome::Changed
            }
            KeyCode::Char('d') => {
                let Some(target) = self.selected_removable_target() else {
                    return ProviderManagerOutcome::Unchanged;
                };
                self.mode = ProviderMode::ConfirmRemove { target };
                ProviderManagerOutcome::Changed
            }
            KeyCode::Char('m') => {
                let Some(provider) = self.selected_provider() else {
                    return ProviderManagerOutcome::Unchanged;
                };
                if !provider.source.is_usable() {
                    return ProviderManagerOutcome::Unchanged;
                }
                ProviderManagerOutcome::SetDefaultModel(provider.info.default_model.clone())
            }
            KeyCode::Char('q') if self.first_run => ProviderManagerOutcome::Quit,
            KeyCode::Char('p') => self.open_key_entry(),
            KeyCode::Enter => {
                if self.is_quit_row() {
                    return ProviderManagerOutcome::Quit;
                }
                let target = match self.tab {
                    ProviderTab::Providers => self
                        .selected_provider()
                        .map(|p| KeyTarget::Provider(p.info.id.clone())),
                    ProviderTab::Accounts => self
                        .selected_account()
                        .map(|a| KeyTarget::Account(a.name.clone())),
                };
                let Some(target) = target else {
                    return ProviderManagerOutcome::Unchanged;
                };
                // A provider with its own OAuth app signs in via the browser,
                // not a pasted key.
                if self.tab == ProviderTab::Providers
                    && self
                        .selected_provider()
                        .is_some_and(|p| p.info.auth.is_some())
                {
                    let (pid, pname) = {
                        let provider = self.selected_provider().expect("checked above");
                        (provider.info.id.clone(), provider.info.name.clone())
                    };
                    self.mode = ProviderMode::SigningIn {
                        provider_id: pid.clone(),
                        provider_name: pname,
                        auth_url: None,
                        editor: LineEditor::default(),
                    };
                    return ProviderManagerOutcome::OAuthLogin { provider_id: pid };
                }
                self.mode = ProviderMode::EnteringKey {
                    target,
                    editor: LineEditor::default(),
                    error: None,
                };
                ProviderManagerOutcome::Changed
            }
            KeyCode::Esc if !self.first_run => ProviderManagerOutcome::Close,
            _ => ProviderManagerOutcome::Unchanged,
        }
    }

    fn set_tab(&mut self, tab: ProviderTab) -> ProviderManagerOutcome {
        if self.tab == tab {
            return ProviderManagerOutcome::Unchanged;
        }
        self.tab = tab;
        self.window.active_tab = match self.tab {
            ProviderTab::Providers => 0,
            ProviderTab::Accounts => 1,
        };
        self.selected = 0;
        self.notice = None;
        ProviderManagerOutcome::Changed
    }

    /// Open masked key entry for the selected row, even if that provider
    /// would otherwise start a browser OAuth flow on Enter.
    fn open_key_entry(&mut self) -> ProviderManagerOutcome {
        if self.is_quit_row() {
            return ProviderManagerOutcome::Unchanged;
        }
        let target = match self.tab {
            ProviderTab::Providers => self
                .selected_provider()
                .map(|p| KeyTarget::Provider(p.info.id.clone())),
            ProviderTab::Accounts => self
                .selected_account()
                .map(|a| KeyTarget::Account(a.name.clone())),
        };
        let Some(target) = target else {
            return ProviderManagerOutcome::Unchanged;
        };
        self.mode = ProviderMode::EnteringKey {
            target,
            editor: LineEditor::default(),
            error: None,
        };
        ProviderManagerOutcome::Changed
    }

    /// Mouse on the first-run stacked layout. `row_rects` is one per
    /// [`Self::browse_menu_items`] row; `tab_rects` is `[Providers, Accounts]`.
    pub fn handle_mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        row_rects: &[Rect],
        tab_rects: &[Rect],
    ) -> ProviderManagerOutcome {
        if !matches!(self.mode, ProviderMode::Browse) {
            return match (&self.mode, kind) {
                (
                    ProviderMode::OfferDefaultModel { .. },
                    MouseEventKind::Down(MouseButton::Left),
                ) => self.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                (ProviderMode::ConfirmRemove { .. }, MouseEventKind::Down(MouseButton::Left)) => {
                    self.handle_key(&KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
                }
                _ => ProviderManagerOutcome::Unchanged,
            };
        }
        let pos = Position::new(column, row);
        match kind {
            MouseEventKind::Moved => {
                for (i, rect) in row_rects.iter().enumerate() {
                    if rect.contains(pos) && self.selected != i {
                        self.selected = i;
                        return ProviderManagerOutcome::Changed;
                    }
                }
                ProviderManagerOutcome::Unchanged
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if tab_rects.first().is_some_and(|r| r.contains(pos)) {
                    return self.set_tab(ProviderTab::Providers);
                }
                if tab_rects.get(1).is_some_and(|r| r.contains(pos)) {
                    return self.set_tab(ProviderTab::Accounts);
                }
                for (i, rect) in row_rects.iter().enumerate() {
                    if rect.contains(pos) {
                        self.selected = i;
                        return self.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                    }
                }
                ProviderManagerOutcome::Unchanged
            }
            _ => ProviderManagerOutcome::Unchanged,
        }
    }

    pub fn handle_paste(&mut self, text: &str) -> ProviderManagerOutcome {
        match &mut self.mode {
            ProviderMode::EnteringKey { editor, .. } | ProviderMode::SigningIn { editor, .. } => {
                let remaining = MAX_KEY_BYTES.saturating_sub(editor.text().len());
                from_line_edit(editor.insert_paste_with_byte_limit(text, remaining))
            }
            ProviderMode::NamingAccount { editor, .. } => {
                let remaining = MAX_ACCOUNT_NAME_BYTES.saturating_sub(editor.text().len());
                from_line_edit(editor.insert_paste_with_byte_limit(text, remaining))
            }
            _ => ProviderManagerOutcome::Unchanged,
        }
    }

    fn selected_removable_target(&self) -> Option<KeyTarget> {
        match self.tab {
            ProviderTab::Providers => self.selected_provider().and_then(|p| {
                matches!(
                    p.source,
                    CredentialSource::Stored | CredentialSource::StoredAndEnv
                )
                .then(|| KeyTarget::Provider(p.info.id.clone()))
            }),
            ProviderTab::Accounts => self
                .selected_account()
                .filter(|a| a.has_stored_key)
                .map(|a| KeyTarget::Account(a.name.clone())),
        }
    }

    fn handle_signing_in_key(&mut self, key: &KeyEvent) -> ProviderManagerOutcome {
        match key.code {
            KeyCode::Esc => {
                self.mode = ProviderMode::Browse;
                ProviderManagerOutcome::CancelOAuth
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let ProviderMode::SigningIn { editor, .. } = &self.mode else {
                    unreachable!("handle_signing_in_key called outside SigningIn");
                };
                let value = editor.text().trim().to_string();
                if value.is_empty() {
                    ProviderManagerOutcome::Unchanged
                } else {
                    ProviderManagerOutcome::SubmitOAuthCode(value)
                }
            }
            _ => {
                let ProviderMode::SigningIn { editor, .. } = &mut self.mode else {
                    unreachable!("handle_signing_in_key called outside SigningIn");
                };
                let remaining = MAX_KEY_BYTES.saturating_sub(editor.text().len());
                let outcome =
                    editor.handle_key_with_insert_policy(key, |c| c.len_utf8() <= remaining);
                from_line_edit(outcome)
            }
        }
    }

    fn handle_entering_key(&mut self, key: &KeyEvent) -> ProviderManagerOutcome {
        let ProviderMode::EnteringKey {
            target,
            editor,
            error,
        } = &mut self.mode
        else {
            unreachable!("handle_entering_key called outside EnteringKey");
        };
        if crate::input::key::is_paste_key(key) {
            return crate::clipboard::system_clipboard_get().map_or(
                ProviderManagerOutcome::Unchanged,
                |text| {
                    let remaining = MAX_KEY_BYTES.saturating_sub(editor.text().len());
                    from_line_edit(editor.insert_paste_with_byte_limit(&text, remaining))
                },
            );
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && !crate::input::key::is_altgr(key.modifiers)
            && matches!(key.code, KeyCode::Char('c' | 'd' | 'q'))
        {
            self.mode = ProviderMode::Browse;
            return ProviderManagerOutcome::Changed;
        }
        match key.code {
            KeyCode::Enter if key.modifiers.is_empty() => {
                let value = editor.text().trim().to_string();
                if value.is_empty() {
                    *error = Some("key cannot be empty".to_string());
                    return ProviderManagerOutcome::Changed;
                }
                let target = target.clone();
                self.mode = ProviderMode::Verifying {
                    target: target.clone(),
                };
                ProviderManagerOutcome::StoreKey { target, key: value }
            }
            KeyCode::Esc => {
                self.mode = ProviderMode::Browse;
                ProviderManagerOutcome::Changed
            }
            _ => {
                let remaining = MAX_KEY_BYTES.saturating_sub(editor.text().len());
                let outcome =
                    editor.handle_key_with_insert_policy(key, |c| c.len_utf8() <= remaining);
                from_line_edit(outcome)
            }
        }
    }

    fn handle_naming_account_key(&mut self, key: &KeyEvent) -> ProviderManagerOutcome {
        let ProviderMode::NamingAccount { editor, error } = &mut self.mode else {
            unreachable!("handle_naming_account_key called outside NamingAccount");
        };
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && !crate::input::key::is_altgr(key.modifiers)
            && matches!(key.code, KeyCode::Char('c' | 'd' | 'q'))
        {
            self.mode = ProviderMode::Browse;
            return ProviderManagerOutcome::Changed;
        }
        match key.code {
            KeyCode::Enter if key.modifiers.is_empty() => {
                let name = editor.text().trim().to_string();
                if name.is_empty() {
                    *error = Some("name cannot be empty".to_string());
                    return ProviderManagerOutcome::Changed;
                }
                self.mode = ProviderMode::EnteringKey {
                    target: KeyTarget::Account(name),
                    editor: LineEditor::default(),
                    error: None,
                };
                ProviderManagerOutcome::Changed
            }
            KeyCode::Esc => {
                self.mode = ProviderMode::Browse;
                ProviderManagerOutcome::Changed
            }
            _ => {
                let remaining = MAX_ACCOUNT_NAME_BYTES.saturating_sub(editor.text().len());
                let outcome =
                    editor.handle_key_with_insert_policy(key, |c| c.len_utf8() <= remaining);
                from_line_edit(outcome)
            }
        }
    }
}

fn from_line_edit(outcome: LineEditOutcome) -> ProviderManagerOutcome {
    match outcome {
        LineEditOutcome::TextChanged
        | LineEditOutcome::CursorChanged
        | LineEditOutcome::HandledNoChange => ProviderManagerOutcome::Changed,
        LineEditOutcome::Unhandled => ProviderManagerOutcome::Unchanged,
    }
}

fn target_label(target: &KeyTarget) -> String {
    match target {
        KeyTarget::Provider(id) => {
            bcode_models::provider(id).map_or_else(|| id.clone(), |p| p.name.clone())
        }
        KeyTarget::Account(name) => name.clone(),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn source_badge(source: CredentialSource) -> (&'static str, bool) {
    match source {
        CredentialSource::Stored => ("configured", false),
        CredentialSource::Env => ("from environment", false),
        CredentialSource::StoredAndEnv => ("from environment (stored key overridden)", false),
        CredentialSource::None => ("not configured", true),
    }
}

fn provider_row_status(row: &ProviderStatus) -> String {
    match row.source {
        CredentialSource::Stored => "configured".to_string(),
        CredentialSource::Env | CredentialSource::StoredAndEnv => "from environment".to_string(),
        CredentialSource::None if row.info.auth.is_some() => {
            match row.info.auth.as_ref().map(|a| &a.profile) {
                Some(bcode_models::ProviderOAuthProfile::Chatgpt) => {
                    "ChatGPT or API key".to_string()
                }
                Some(bcode_models::ProviderOAuthProfile::Oidc) | None => {
                    "sign in or API key".to_string()
                }
            }
        }
        CredentialSource::None => "paste a key".to_string(),
    }
}

fn account_row_status(row: &AccountStatus) -> String {
    if row.has_stored_key {
        "configured".to_string()
    } else if let Some(env) = &row.env_key {
        format!("from {env}")
    } else if row.kind == AccountKind::Command {
        "external command".to_string()
    } else {
        "paste a key".to_string()
    }
}

fn cols(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn render_provider_row(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    name: &str,
    name_width: usize,
    detail: &str,
    dim_detail: bool,
    selected: bool,
) {
    let name_style = if selected {
        Style::default()
            .fg(theme.accent_user)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_primary)
    };
    let detail_style = if dim_detail {
        Style::default().fg(theme.gray_dim)
    } else {
        Style::default().fg(theme.gray_bright)
    };
    let marker = if selected { "\u{203a} " } else { "  " };
    let line = Line::from(vec![
        Span::styled(marker, name_style),
        Span::styled(format!("{name:<name_width$}"), name_style),
        Span::styled("  ", Style::default()),
        Span::styled(detail.to_string(), detail_style),
    ]);
    let line = crate::render::line_utils::truncate_line(line, area.width as usize);
    buf.set_line(area.x, area.y, &line, area.width);
}

/// Render the manager's content (list + any active sub-mode) into `area`.
/// Shared by the full-screen and modal hosts.
pub fn render_provider_manager_content(area: Rect, buf: &mut Buffer, state: &ProviderManagerState) {
    let theme = Theme::current();
    if area.height == 0 || area.width == 0 {
        return;
    }

    let notice_height = u16::from(state.notice.is_some());
    let detail_height = match &state.mode {
        ProviderMode::EnteringKey { .. } | ProviderMode::Verifying { .. } => 4,
        ProviderMode::NamingAccount { .. } => 3,
        ProviderMode::ConfirmRemove { .. } => 2,
        ProviderMode::OfferDefaultModel { .. } => 2,
        ProviderMode::SigningIn { .. } => 2,
        ProviderMode::Browse => 0,
    };
    let [list_area, notice_area, detail_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(notice_height),
        Constraint::Length(detail_height),
    ])
    .areas(area);

    if list_area.width >= 4 && list_area.height >= 3 {
        let list_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.gray_dim))
            .padding(Padding::horizontal(1));
        let list_inner = list_block.inner(list_area);
        list_block.render(list_area, buf);
        render_row_list(list_inner, buf, &theme, state);
    } else {
        render_row_list(list_area, buf, &theme, state);
    }

    if let Some((message, is_error)) = &state.notice {
        let color = if *is_error {
            theme.accent_error
        } else {
            theme.gray_bright
        };
        Paragraph::new(Line::from(Span::styled(
            message.clone(),
            Style::default().fg(color),
        )))
        .render(notice_area, buf);
    }

    render_mode_detail(detail_area, buf, &theme, state);
}

fn render_row_list(area: Rect, buf: &mut Buffer, theme: &Theme, state: &ProviderManagerState) {
    match state.tab {
        ProviderTab::Providers => {
            let name_width = state
                .providers
                .iter()
                .map(|row| cols(&row.info.name))
                .max()
                .unwrap_or(0);
            for (i, row) in state.providers.iter().enumerate() {
                let y = area.y + i as u16;
                if y >= area.y + area.height {
                    break;
                }
                let (badge, dim) = source_badge(row.source);
                render_provider_row(
                    buf,
                    Rect::new(area.x, y, area.width, 1),
                    theme,
                    &row.info.name,
                    name_width,
                    badge,
                    dim,
                    i == state.selected,
                );
            }
            if state.providers.is_empty() {
                Paragraph::new(Line::from(Span::styled(
                    "No providers in the catalog.",
                    Style::default().fg(theme.gray_dim),
                )))
                .render(area, buf);
            }
        }
        ProviderTab::Accounts => {
            if state.accounts.is_empty() {
                Paragraph::new(Line::from(Span::styled(
                    "No named accounts yet. Press \u{2018}a\u{2019} to add one.",
                    Style::default().fg(theme.gray_dim),
                )))
                .render(area, buf);
                return;
            }
            let name_width = state
                .accounts
                .iter()
                .map(|row| cols(&row.name))
                .max()
                .unwrap_or(0);
            for (i, row) in state.accounts.iter().enumerate() {
                let y = area.y + i as u16;
                if y >= area.y + area.height {
                    break;
                }
                let detail = if row.has_stored_key {
                    "configured".to_string()
                } else if let Some(env) = &row.env_key {
                    format!("from {env}")
                } else if row.kind == AccountKind::Command {
                    "external command".to_string()
                } else {
                    "not configured".to_string()
                };
                let dim = !row.has_stored_key
                    && row.env_key.is_none()
                    && row.kind != AccountKind::Command;
                render_provider_row(
                    buf,
                    Rect::new(area.x, y, area.width, 1),
                    theme,
                    &row.name,
                    name_width,
                    &detail,
                    dim,
                    i == state.selected,
                );
            }
        }
    }
}

fn render_mode_detail(area: Rect, buf: &mut Buffer, theme: &Theme, state: &ProviderManagerState) {
    if area.height == 0 {
        return;
    }
    match &state.mode {
        ProviderMode::Browse => {}
        ProviderMode::EnteringKey {
            target,
            editor,
            error,
        } => {
            let [hint_area, input_area, error_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(area.height.saturating_sub(4)),
            ])
            .areas(area);
            let hint = match target {
                KeyTarget::Provider(id) => bcode_models::provider(id).map_or_else(
                    || format!("Paste an API key for {id}:"),
                    |p| {
                        format!(
                            "Paste an API key for {} (or set {} in your environment):",
                            p.name, p.env_key
                        )
                    },
                ),
                KeyTarget::Account(name) => {
                    format!("Paste an API key for account \u{201c}{name}\u{201d}:")
                }
            };
            Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default().fg(theme.gray_bright),
            )))
            .render(hint_area, buf);
            render_masked_input_box(
                input_area,
                buf,
                theme,
                editor.text(),
                editor.cursor_byte(),
                "",
            );
            if let Some(err) = error {
                Paragraph::new(Line::from(Span::styled(
                    err.clone(),
                    Style::default().fg(theme.accent_error),
                )))
                .render(error_area, buf);
            }
        }
        ProviderMode::Verifying { target } => {
            let text = format!("Verifying {}\u{2026}", target_label(target));
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(theme.gray_bright),
            )))
            .render(area, buf);
        }
        ProviderMode::NamingAccount { editor, error } => {
            let [hint_area, input_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(area.height.saturating_sub(1)),
            ])
            .areas(area);
            let hint = error
                .clone()
                .unwrap_or_else(|| "Name this account:".to_string());
            let color = if error.is_some() {
                theme.accent_error
            } else {
                theme.gray_bright
            };
            Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(color))))
                .render(hint_area, buf);
            let viewport = editor.viewport(input_area.width as usize);
            let visible = &editor.text()[viewport.visible_byte_range];
            Paragraph::new(Line::from(Span::styled(
                visible.to_string(),
                Style::default().fg(theme.text_primary),
            )))
            .render(input_area, buf);
        }
        ProviderMode::ConfirmRemove { target } => {
            let text = format!("Remove the stored key for {}? y/N", target_label(target));
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(theme.accent_error),
            )))
            .render(area, buf);
        }
        ProviderMode::OfferDefaultModel { model, .. } => {
            let text = format!("Use {model} as your default model? Y/n");
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(theme.text_primary),
            )))
            .render(area, buf);
        }
        ProviderMode::SigningIn { provider_name, .. } => {
            let text =
                format!("Signing in with {provider_name}\u{2026} complete it in your browser.");
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(theme.gray_bright),
            )))
            .render(area, buf);
        }
    }
}

/// Footer shortcuts for the current mode, for the modal-chrome host.
pub fn provider_manager_shortcuts(state: &ProviderManagerState) -> Vec<Shortcut<'static>> {
    match &state.mode {
        ProviderMode::Browse => {
            let mut shortcuts = vec![
                Shortcut {
                    label: "enter continue",
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: "p paste a key",
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: "d remove",
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: "m set default",
                    clickable: false,
                    id: 0,
                },
                Shortcut {
                    label: "tab switch list",
                    clickable: false,
                    id: 0,
                },
            ];
            if state.tab == ProviderTab::Accounts {
                shortcuts.push(Shortcut {
                    label: "a add account",
                    clickable: false,
                    id: 0,
                });
            }
            if !state.first_run {
                shortcuts.push(Shortcut {
                    label: "esc close",
                    clickable: false,
                    id: 0,
                });
            }
            shortcuts
        }
        ProviderMode::EnteringKey { .. } => vec![
            Shortcut {
                label: "enter save",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "esc cancel",
                clickable: false,
                id: 0,
            },
        ],
        ProviderMode::Verifying { .. } => vec![],
        ProviderMode::SigningIn { .. } => vec![],
        ProviderMode::NamingAccount { .. } => vec![
            Shortcut {
                label: "enter continue",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "esc cancel",
                clickable: false,
                id: 0,
            },
        ],
        ProviderMode::ConfirmRemove { .. } => vec![
            Shortcut {
                label: "y remove",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "n cancel",
                clickable: false,
                id: 0,
            },
        ],
        ProviderMode::OfferDefaultModel { .. } => vec![
            Shortcut {
                label: "enter/y use as default",
                clickable: false,
                id: 0,
            },
            Shortcut {
                label: "n skip",
                clickable: false,
                id: 0,
            },
        ],
    }
}

/// Render the manager full-screen (first run): no border, no close button --
/// there is nowhere else to go until a credential resolves.
pub fn render_provider_manager_fullscreen(
    area: Rect,
    buf: &mut Buffer,
    state: &ProviderManagerState,
) {
    let theme = Theme::current();
    let title = if state.tab == ProviderTab::Providers {
        "Sign in to a provider"
    } else {
        "Named accounts"
    };
    let [title_area, tabs_area, content_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(area);
    Paragraph::new(Line::from(Span::styled(
        title,
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    )))
    .render(title_area, buf);
    render_tab_line(tabs_area, buf, &theme, state.tab);
    render_provider_manager_content(content_area, buf, state);
}

fn render_tab_line(area: Rect, buf: &mut Buffer, theme: &Theme, active: ProviderTab) {
    let _ = render_centered_tabs(area, buf, theme, active);
}

/// Centered Providers / Accounts tabs. Returns a hit rect per tab, in that order.
pub fn render_centered_tabs(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    active: ProviderTab,
) -> Vec<Rect> {
    if area.height == 0 || area.width == 0 {
        return Vec::new();
    }
    let style = |tab: ProviderTab| {
        if tab == active {
            Style::default()
                .fg(theme.accent_user)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray)
        }
    };
    const PROVIDERS: &str = "Providers";
    const ACCOUNTS: &str = "Accounts";
    const GAP: u16 = 4;
    let left_w = cols(PROVIDERS) as u16;
    let right_w = cols(ACCOUNTS) as u16;
    let total = left_w + GAP + right_w;
    let start = area.x + area.width.saturating_sub(total) / 2;
    let providers_rect = Rect {
        x: start,
        y: area.y,
        width: left_w,
        height: 1,
    };
    let accounts_rect = Rect {
        x: start + left_w + GAP,
        y: area.y,
        width: right_w,
        height: 1,
    };
    buf.set_span(
        providers_rect.x,
        providers_rect.y,
        &Span::styled(PROVIDERS, style(ProviderTab::Providers)),
        left_w,
    );
    buf.set_span(
        accounts_rect.x,
        accounts_rect.y,
        &Span::styled(ACCOUNTS, style(ProviderTab::Accounts)),
        right_w,
    );
    vec![providers_rect, accounts_rect]
}

/// Render the manager inside modal chrome (mid-session `/providers`, `/login`).
pub fn render_provider_manager_modal(
    area: Rect,
    buf: &mut Buffer,
    state: &mut ProviderManagerState,
) {
    let theme = Theme::current();
    let shortcuts = provider_manager_shortcuts(state);
    let config = ModalWindowConfig {
        title: "Providers",
        tabs: Some(&["Providers", "Accounts"]),
        shortcuts: &shortcuts,
        sizing: ModalSizing::medium(),
        fold_info: None,
    };
    if let Some(ModalContentArea { content, .. }) =
        render_modal_window(buf, area, &mut state.window, &config, &theme)
    {
        render_provider_manager_content(content, buf, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn state() -> ProviderManagerState {
        let dir = tempfile::tempdir().expect("tempdir");
        // Leaked on purpose: the state only needs `home` for its lifetime;
        // tests don't reload from disk after construction unless noted.
        let home = Box::leak(Box::new(dir)).path();
        ProviderManagerState::open(home)
    }

    #[test]
    fn enter_on_a_provider_opens_masked_key_entry() {
        let mut s = state();
        let outcome = s.handle_key(&key(KeyCode::Enter));
        assert_eq!(outcome, ProviderManagerOutcome::Changed);
        let ProviderMode::EnteringKey { editor, .. } = &s.mode else {
            panic!("expected EnteringKey, got {:?}", s.mode);
        };
        assert!(editor.text().is_empty());

        for c in "sk-test-1234".chars() {
            s.handle_key(&key(KeyCode::Char(c)));
        }
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        render_provider_manager_content(area, &mut buf, &s);
        let rendered: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains('\u{2022}'),
            "expected masked dots:\n{rendered}"
        );
        assert!(
            rendered.contains("1234"),
            "expected last 4 chars visible:\n{rendered}"
        );
    }

    #[test]
    fn blank_key_is_refused_in_the_view_not_on_disk() {
        let mut s = state();
        s.handle_key(&key(KeyCode::Enter));
        let outcome = s.handle_key(&key(KeyCode::Enter));
        assert_eq!(outcome, ProviderManagerOutcome::Changed);
        let ProviderMode::EnteringKey { error, .. } = &s.mode else {
            panic!("expected still EnteringKey, got {:?}", s.mode);
        };
        assert!(error.is_some(), "blank submit must set a view-side error");
    }

    #[test]
    fn remove_requires_confirmation() {
        let mut s = state();
        // Nothing stored yet: 'd' on a credential-less row is a no-op.
        assert_eq!(
            s.handle_key(&key(KeyCode::Char('d'))),
            ProviderManagerOutcome::Unchanged
        );
        assert!(matches!(s.mode, ProviderMode::Browse));

        // Simulate a stored provider credential landing, then 'd' opens confirm.
        s.providers[0].source = CredentialSource::Stored;
        let outcome = s.handle_key(&key(KeyCode::Char('d')));
        assert_eq!(outcome, ProviderManagerOutcome::Changed);
        assert!(matches!(s.mode, ProviderMode::ConfirmRemove { .. }));

        // 'n' cancels without removing.
        let outcome = s.handle_key(&key(KeyCode::Char('n')));
        assert_eq!(outcome, ProviderManagerOutcome::Changed);
        assert!(matches!(s.mode, ProviderMode::Browse));

        // 'd' then 'y' emits RemoveKey.
        s.handle_key(&key(KeyCode::Char('d')));
        let target = KeyTarget::Provider(s.providers[0].info.id.clone());
        let outcome = s.handle_key(&key(KeyCode::Char('y')));
        assert_eq!(outcome, ProviderManagerOutcome::RemoveKey { target });
    }

    #[test]
    fn escape_backs_out_one_level_then_closes() {
        let mut s = state();
        s.first_run = false;
        s.handle_key(&key(KeyCode::Enter));
        assert!(matches!(s.mode, ProviderMode::EnteringKey { .. }));
        let outcome = s.handle_key(&key(KeyCode::Esc));
        assert_eq!(outcome, ProviderManagerOutcome::Changed);
        assert!(
            matches!(s.mode, ProviderMode::Browse),
            "esc backs out of key entry first"
        );

        let outcome = s.handle_key(&key(KeyCode::Esc));
        assert_eq!(
            outcome,
            ProviderManagerOutcome::Close,
            "esc from Browse closes"
        );
    }

    #[test]
    fn first_run_escape_never_closes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = ProviderManagerState::first_run(dir.path());
        assert!(s.first_run);
        let outcome = s.handle_key(&key(KeyCode::Esc));
        assert_eq!(outcome, ProviderManagerOutcome::Unchanged);
    }

    #[test]
    fn first_run_escape_during_oauth_cancels() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = ProviderManagerState::first_run(dir.path());
        s.mode = ProviderMode::SigningIn {
            provider_id: "p".into(),
            provider_name: "P".into(),
            auth_url: None,
            editor: LineEditor::default(),
        };
        assert_eq!(
            s.handle_key(&key(KeyCode::Esc)),
            ProviderManagerOutcome::CancelOAuth
        );
        assert!(matches!(s.mode, ProviderMode::Browse));
    }

    #[test]
    fn first_run_quit_row_and_q_leave_the_app() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = ProviderManagerState::first_run(dir.path());
        let items = s.browse_menu_items();
        assert_eq!(items.last().map(|(_, n)| n.as_str()), Some("Quit"));
        assert_eq!(s.row_count(), items.len());
        s.selected = s.row_count() - 1;
        assert_eq!(
            s.handle_key(&key(KeyCode::Enter)),
            ProviderManagerOutcome::Quit
        );

        let mut s = ProviderManagerState::first_run(dir.path());
        assert_eq!(
            s.handle_key(&key(KeyCode::Char('q'))),
            ProviderManagerOutcome::Quit
        );
    }

    #[test]
    fn p_opens_key_entry_even_when_the_provider_has_oauth() {
        let mut s = state();
        let Some(idx) = s.providers.iter().position(|p| p.info.auth.is_some()) else {
            return;
        };
        s.selected = idx;
        let enter = s.handle_key(&key(KeyCode::Enter));
        assert!(
            matches!(enter, ProviderManagerOutcome::OAuthLogin { .. }),
            "Enter on an OAuth provider starts the browser flow: {enter:?}"
        );
        s.mode = ProviderMode::Browse;
        let outcome = s.handle_key(&key(KeyCode::Char('p')));
        assert_eq!(outcome, ProviderManagerOutcome::Changed);
        assert!(
            matches!(s.mode, ProviderMode::EnteringKey { .. }),
            "p must open key entry, got {:?}",
            s.mode
        );
    }

    #[test]
    fn click_on_a_row_matches_enter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = ProviderManagerState::first_run(dir.path());
        let n = s.list_len();
        let row_rects: Vec<Rect> = (0..=n)
            .map(|i| Rect::new(10, 10 + i as u16, 40, 1))
            .collect();
        let quit = s.handle_mouse(
            MouseEventKind::Down(MouseButton::Left),
            12,
            10 + n as u16,
            &row_rects,
            &[],
        );
        assert_eq!(quit, ProviderManagerOutcome::Quit);

        s.selected = 0;
        s.mode = ProviderMode::Browse;
        let hover = s.handle_mouse(MouseEventKind::Moved, 12, 11, &row_rects, &[]);
        assert_eq!(hover, ProviderManagerOutcome::Changed);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn successful_store_in_an_auth_gated_manager_offers_the_default_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = ProviderManagerState::first_run(dir.path());
        let id = s.providers[0].info.id.clone();
        let default_model = s.providers[0].info.default_model.clone();
        provider_setup::store_provider_credential(dir.path(), &id, "sk-test-1234").expect("store");
        s.mode = ProviderMode::Verifying {
            target: KeyTarget::Provider(id.clone()),
        };
        s.finish_store(dir.path(), &KeyTarget::Provider(id.clone()), Some(Ok(())));
        let ProviderMode::OfferDefaultModel { provider_id, model } = &s.mode else {
            panic!("expected OfferDefaultModel, got {:?}", s.mode);
        };
        assert_eq!(provider_id, &id);
        assert_eq!(model, &default_model);

        let outcome = s.handle_key(&key(KeyCode::Enter));
        assert_eq!(
            outcome,
            ProviderManagerOutcome::SetDefaultModel(default_model)
        );
        assert!(matches!(s.mode, ProviderMode::Browse));
    }

    #[test]
    fn declining_the_default_model_offer_still_signals_ready() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = ProviderManagerState::first_run(dir.path());
        let id = s.providers[0].info.id.clone();
        provider_setup::store_provider_credential(dir.path(), &id, "sk-test-1234").expect("store");
        s.mode = ProviderMode::Verifying {
            target: KeyTarget::Provider(id.clone()),
        };
        s.finish_store(dir.path(), &KeyTarget::Provider(id), None);
        assert!(matches!(s.mode, ProviderMode::OfferDefaultModel { .. }));

        let outcome = s.handle_key(&key(KeyCode::Char('n')));
        assert_eq!(outcome, ProviderManagerOutcome::DismissDefaultOffer);
        assert!(matches!(s.mode, ProviderMode::Browse));
    }

    #[test]
    fn a_management_only_manager_never_offers_the_default_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = ProviderManagerState::open(dir.path());
        let id = s.providers[0].info.id.clone();
        provider_setup::store_provider_credential(dir.path(), &id, "sk-test-1234").expect("store");
        s.mode = ProviderMode::Verifying {
            target: KeyTarget::Provider(id.clone()),
        };
        s.finish_store(dir.path(), &KeyTarget::Provider(id), Some(Ok(())));
        assert!(
            matches!(s.mode, ProviderMode::Browse),
            "auth_gated=false must not trigger the offer: {:?}",
            s.mode
        );
    }

    #[test]
    fn provider_rows_never_hardcode_a_provider_name() {
        let s = state();
        let area = Rect::new(0, 0, 80, 40);
        let mut buf = Buffer::empty(area);
        render_provider_manager_content(area, &mut buf, &s);
        let rendered: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for provider in bcode_models::providers() {
            assert!(
                rendered.contains(&provider.name),
                "expected {} to appear in the rendered provider list:\n{rendered}",
                provider.name
            );
        }
    }
}
