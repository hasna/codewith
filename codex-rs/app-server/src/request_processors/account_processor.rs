use super::account_rate_limit_resets;
use super::*;
use codex_app_server_protocol::ConsumeAccountRateLimitResetCreditOutcome;

// Duration before a browser ChatGPT login attempt is abandoned.
const LOGIN_CHATGPT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const ACCOUNT_TOKEN_USAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 10);
// The override is intentionally available only in debug builds, matching the login path below.
#[cfg(debug_assertions)]
const LOGIN_ISSUER_OVERRIDE_ENV_VAR: &str = "CODEX_APP_SERVER_LOGIN_ISSUER";

enum ActiveLogin {
    Browser {
        shutdown_handle: ShutdownHandle,
        login_id: Uuid,
    },
    DeviceCode {
        cancel: CancellationToken,
        login_id: Uuid,
    },
}

impl ActiveLogin {
    fn login_id(&self) -> Uuid {
        match self {
            ActiveLogin::Browser { login_id, .. } | ActiveLogin::DeviceCode { login_id, .. } => {
                *login_id
            }
        }
    }

    fn cancel(&self) {
        match self {
            ActiveLogin::Browser {
                shutdown_handle, ..
            } => shutdown_handle.shutdown(),
            ActiveLogin::DeviceCode { cancel, .. } => cancel.cancel(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum CancelLoginError {
    NotFound,
}

enum RefreshTokenRequestOutcome {
    NotAttemptedOrSucceeded,
    FailedTransiently,
    FailedPermanently,
}

impl Drop for ActiveLogin {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone)]
pub(crate) struct AccountRequestProcessor {
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    config_manager: ConfigManager,
    active_login: Arc<Mutex<Option<ActiveLogin>>>,
}

impl AccountRequestProcessor {
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        config_manager: ConfigManager,
    ) -> Self {
        Self {
            auth_manager,
            thread_manager,
            outgoing,
            config,
            config_manager,
            active_login: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) async fn login_account(
        &self,
        request_id: ConnectionRequestId,
        params: LoginAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.login_v2(request_id, params).await.map(|()| None)
    }

    pub(crate) async fn logout_account(
        &self,
        request_id: ConnectionRequestId,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.logout_v2(request_id).await.map(|()| None)
    }

    pub(crate) async fn cancel_login_account(
        &self,
        params: CancelLoginAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.cancel_login_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn list_auth_profiles(
        &self,
        params: AuthProfileListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let AuthProfileListParams { cursor, limit } = params;
        let mut profiles = codex_login::list_auth_profiles(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
        )
        .map_err(|err| invalid_request(format!("failed to list auth profiles: {err}")))?;
        if let Some(selected_auth_profile) = self.auth_manager.selected_auth_profile() {
            for profile in &mut profiles {
                profile.active = profile.name == selected_auth_profile;
            }
        }

        let total = profiles.len();
        let start = match cursor {
            Some(cursor) => cursor
                .parse::<usize>()
                .map_err(|_| invalid_request(format!("invalid cursor: {cursor}")))?,
            None => 0,
        };
        if start > total {
            return Err(invalid_request(format!(
                "cursor {start} exceeds total auth profiles {total}"
            )));
        }
        let effective_limit = limit.unwrap_or(total as u32).max(1) as usize;
        let end = start.saturating_add(effective_limit).min(total);
        let next_cursor = (end < total).then_some(end.to_string());

        Ok(Some(
            AuthProfileListResponse {
                data: profiles[start..end]
                    .iter()
                    .cloned()
                    .map(Self::auth_profile_summary)
                    .collect(),
                next_cursor,
            }
            .into(),
        ))
    }

    pub(crate) async fn save_current_auth_profile(
        &self,
        params: AuthProfileSaveCurrentParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let profile = codex_login::save_current_auth_profile(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
            &params.name,
        )
        .map_err(|err| invalid_request(format!("failed to save auth profile: {err}")))?;

        self.auth_manager
            .switch_auth_profile(Some(profile.name.clone()))
            .await
            .map_err(|err| invalid_request(format!("failed to reload auth profile: {err}")))?;

        self.finish_auth_profile_change().await;

        Ok(Some(
            AuthProfileSaveCurrentResponse {
                profile: Self::auth_profile_summary(profile),
            }
            .into(),
        ))
    }

    pub(crate) async fn switch_auth_profile(
        &self,
        params: AuthProfileSwitchParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let profile = codex_login::switch_auth_profile(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
            &params.name,
        )
        .map_err(|err| invalid_request(format!("failed to switch auth profile: {err}")))?;

        self.auth_manager
            .switch_auth_profile(Some(params.name))
            .await
            .map_err(|err| invalid_request(format!("failed to reload auth profile: {err}")))?;

        self.finish_auth_profile_change().await;

        Ok(Some(
            AuthProfileSwitchResponse {
                profile: Self::auth_profile_summary(profile),
            }
            .into(),
        ))
    }

    pub(crate) async fn get_account(
        &self,
        params: GetAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_auth_status(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_auth_status_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_account_rate_limits(
        &self,
        params: GetAccountRateLimitsParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_rate_limits_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn consume_account_rate_limit_reset_credit(
        &self,
        params: ConsumeAccountRateLimitResetCreditParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.consume_account_rate_limit_reset_credit_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_account_token_usage(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_token_usage_response()
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn send_add_credits_nudge_email(
        &self,
        params: SendAddCreditsNudgeEmailParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.send_add_credits_nudge_email_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn cancel_active_login(&self) {
        let mut guard = self.active_login.lock().await;
        if let Some(active_login) = guard.take() {
            drop(active_login);
        }
    }

    pub(crate) fn clear_external_auth(&self) {
        self.auth_manager.clear_external_auth();
    }

    fn current_account_updated_notification(&self) -> AccountUpdatedNotification {
        let auth = self.auth_manager.auth_cached();
        AccountUpdatedNotification {
            auth_mode: auth.as_ref().map(CodexAuth::api_auth_mode),
            plan_type: auth.as_ref().and_then(CodexAuth::account_plan_type),
        }
    }

    fn auth_profile_summary(profile: codex_login::AuthProfile) -> AuthProfileSummary {
        AuthProfileSummary {
            name: profile.name,
            subscription_provider: match profile.subscription_provider {
                codex_login::AuthProfileSubscriptionProvider::ChatGpt => {
                    AuthProfileSubscriptionProvider::Chatgpt
                }
                codex_login::AuthProfileSubscriptionProvider::ClaudeAi => {
                    AuthProfileSubscriptionProvider::ClaudeAi
                }
                codex_login::AuthProfileSubscriptionProvider::Cursor => {
                    AuthProfileSubscriptionProvider::Cursor
                }
                codex_login::AuthProfileSubscriptionProvider::Grok => {
                    AuthProfileSubscriptionProvider::Grok
                }
            },
            auth_mode: profile.auth_mode,
            email: profile.email,
            account_id: profile.account_id,
            plan: profile.plan,
            active: profile.active,
        }
    }

    async fn finish_auth_profile_change(&self) {
        self.config_manager.replace_cloud_config_bundle_loader(
            self.auth_manager.clone(),
            self.config.chatgpt_base_url.clone(),
        );
        self.config_manager
            .sync_default_client_residency_requirement()
            .await;

        Self::maybe_refresh_remote_installed_plugins_cache_for_current_config(
            &self.config_manager,
            &self.thread_manager,
            self.auth_manager.auth_cached(),
        )
        .await;

        self.outgoing
            .send_server_notification(ServerNotification::AccountUpdated(
                self.current_account_updated_notification(),
            ))
            .await;
    }

    async fn maybe_refresh_remote_installed_plugins_cache_for_current_config(
        config_manager: &ConfigManager,
        thread_manager: &Arc<ThreadManager>,
        auth: Option<CodexAuth>,
    ) {
        match config_manager
            .load_latest_config(/*fallback_cwd*/ None)
            .await
        {
            Ok(config) => {
                let refresh_thread_manager = Arc::clone(thread_manager);
                let refresh_config_manager = config_manager.clone();
                thread_manager
                    .plugins_manager()
                    .maybe_start_remote_installed_plugins_cache_refresh(
                        &config.plugins_config_input(),
                        auth,
                        Some(Arc::new(move || {
                            Self::spawn_effective_plugins_changed_task(
                                Arc::clone(&refresh_thread_manager),
                                refresh_config_manager.clone(),
                            );
                        })),
                    );
            }
            Err(err) => {
                warn!(
                    "failed to reload config after account changed, skipping remote installed plugins cache refresh: {err}"
                );
            }
        }
    }

    fn spawn_effective_plugins_changed_task(
        thread_manager: Arc<ThreadManager>,
        config_manager: ConfigManager,
    ) {
        tokio::spawn(async move {
            thread_manager.plugins_manager().clear_cache();
            thread_manager.skills_manager().clear_cache();
            if thread_manager.list_thread_ids().await.is_empty() {
                return;
            }
            crate::mcp_refresh::queue_best_effort_refresh(&thread_manager, &config_manager).await;
        });
    }

    async fn login_v2(
        &self,
        request_id: ConnectionRequestId,
        params: LoginAccountParams,
    ) -> Result<(), JSONRPCErrorError> {
        match params {
            LoginAccountParams::ApiKey { api_key } => {
                self.login_api_key_v2(request_id, LoginApiKeyParams { api_key })
                    .await;
            }
            LoginAccountParams::Chatgpt {
                codex_streamlined_login,
            } => {
                self.login_chatgpt_v2(request_id, codex_streamlined_login)
                    .await;
            }
            LoginAccountParams::ChatgptDeviceCode => {
                self.login_chatgpt_device_code_v2(request_id).await;
            }
            LoginAccountParams::ChatgptAuthTokens {
                access_token,
                chatgpt_account_id,
                chatgpt_plan_type,
            } => {
                self.login_chatgpt_auth_tokens(
                    request_id,
                    access_token,
                    chatgpt_account_id,
                    chatgpt_plan_type,
                )
                .await;
            }
        }
        Ok(())
    }

    fn external_auth_active_error(&self) -> JSONRPCErrorError {
        invalid_request(
            "External auth is active. Use account/login/start (chatgptAuthTokens) to update it or account/logout to clear it.",
        )
    }

    fn auth_storage_home(&self) -> Result<PathBuf, JSONRPCErrorError> {
        account_auth_storage_home(&self.config)
    }

    async fn login_api_key_common(
        &self,
        params: &LoginApiKeyParams,
    ) -> std::result::Result<(), JSONRPCErrorError> {
        if self.auth_manager.is_external_chatgpt_auth_active() {
            return Err(self.external_auth_active_error());
        }

        if matches!(
            self.config.forced_login_method,
            Some(ForcedLoginMethod::Chatgpt)
        ) {
            return Err(invalid_request(
                "API key login is disabled. Use ChatGPT login instead.",
            ));
        }

        // Cancel any active login attempt.
        {
            let mut guard = self.active_login.lock().await;
            if let Some(active) = guard.take() {
                drop(active);
            }
        }

        let auth_storage_home = self.auth_storage_home()?;
        match login_with_api_key(
            &auth_storage_home,
            &params.api_key,
            self.config.cli_auth_credentials_store_mode,
        ) {
            Ok(()) => {
                self.auth_manager.reload().await;
                Ok(())
            }
            Err(err) => Err(internal_error(format!("failed to save api key: {err}"))),
        }
    }

    async fn login_api_key_v2(&self, request_id: ConnectionRequestId, params: LoginApiKeyParams) {
        let result = self
            .login_api_key_common(&params)
            .await
            .map(|()| LoginAccountResponse::ApiKey {});
        let logged_in = result.is_ok();
        self.outgoing.send_result(request_id, result).await;

        if logged_in {
            self.send_login_success_notifications(/*login_id*/ None)
                .await;
        }
    }

    // Build options for a ChatGPT login attempt; performs validation.
    async fn login_chatgpt_common(
        &self,
        codex_streamlined_login: bool,
    ) -> std::result::Result<LoginServerOptions, JSONRPCErrorError> {
        let config = self.config.as_ref();

        if self.auth_manager.is_external_chatgpt_auth_active() {
            return Err(self.external_auth_active_error());
        }

        if matches!(config.forced_login_method, Some(ForcedLoginMethod::Api)) {
            return Err(invalid_request(
                "ChatGPT login is disabled. Use API key login instead.",
            ));
        }

        let auth_storage_home = self.auth_storage_home()?;
        let opts = LoginServerOptions {
            open_browser: false,
            codex_streamlined_login,
            ..LoginServerOptions::new(
                auth_storage_home,
                CLIENT_ID.to_string(),
                config.forced_chatgpt_workspace_id.clone(),
                config.cli_auth_credentials_store_mode,
            )
        };
        #[cfg(debug_assertions)]
        let opts = {
            let mut opts = opts;
            if let Ok(issuer) = std::env::var(LOGIN_ISSUER_OVERRIDE_ENV_VAR)
                && !issuer.trim().is_empty()
            {
                opts.issuer = issuer;
            }
            opts
        };

        Ok(opts)
    }

    fn login_chatgpt_device_code_start_error(err: IoError) -> JSONRPCErrorError {
        let is_not_found = err.kind() == std::io::ErrorKind::NotFound;
        if is_not_found {
            invalid_request(err.to_string())
        } else {
            internal_error(format!("failed to request device code: {err}"))
        }
    }

    async fn login_chatgpt_v2(
        &self,
        request_id: ConnectionRequestId,
        codex_streamlined_login: bool,
    ) {
        let result = self.login_chatgpt_response(codex_streamlined_login).await;
        self.outgoing.send_result(request_id, result).await;
    }

    async fn login_chatgpt_response(
        &self,
        codex_streamlined_login: bool,
    ) -> Result<LoginAccountResponse, JSONRPCErrorError> {
        let opts = self.login_chatgpt_common(codex_streamlined_login).await?;
        let server = run_login_server(opts)
            .map_err(|err| internal_error(format!("failed to start login server: {err}")))?;
        let login_id = Uuid::new_v4();
        let shutdown_handle = server.cancel_handle();

        // Replace active login if present.
        {
            let mut guard = self.active_login.lock().await;
            if let Some(existing) = guard.take() {
                drop(existing);
            }
            *guard = Some(ActiveLogin::Browser {
                shutdown_handle: shutdown_handle.clone(),
                login_id,
            });
        }

        let outgoing_clone = self.outgoing.clone();
        let config_manager = self.config_manager.clone();
        let thread_manager = Arc::clone(&self.thread_manager);
        let chatgpt_base_url = self.config.chatgpt_base_url.clone();
        let active_login = self.active_login.clone();
        let auth_url = server.auth_url.clone();
        tokio::spawn(async move {
            let (success, error_msg) = match tokio::time::timeout(
                LOGIN_CHATGPT_TIMEOUT,
                server.block_until_done(),
            )
            .await
            {
                Ok(Ok(())) => (true, None),
                Ok(Err(err)) => (false, Some(format!("Login server error: {err}"))),
                Err(_elapsed) => {
                    shutdown_handle.shutdown();
                    (false, Some("Login timed out".to_string()))
                }
            };

            Self::send_chatgpt_login_completion_notifications(
                &outgoing_clone,
                config_manager,
                thread_manager,
                chatgpt_base_url,
                login_id,
                success,
                error_msg,
            )
            .await;

            // Clear the active login if it matches this attempt. It may have been replaced or cancelled.
            let mut guard = active_login.lock().await;
            if guard.as_ref().map(ActiveLogin::login_id) == Some(login_id) {
                *guard = None;
            }
        });

        Ok(LoginAccountResponse::Chatgpt {
            login_id: login_id.to_string(),
            auth_url,
        })
    }

    async fn login_chatgpt_device_code_v2(&self, request_id: ConnectionRequestId) {
        let result = self.login_chatgpt_device_code_response().await;
        self.outgoing.send_result(request_id, result).await;
    }

    async fn login_chatgpt_device_code_response(
        &self,
    ) -> Result<LoginAccountResponse, JSONRPCErrorError> {
        let opts = self
            .login_chatgpt_common(/*codex_streamlined_login*/ false)
            .await?;
        let device_code = request_device_code(&opts)
            .await
            .map_err(Self::login_chatgpt_device_code_start_error)?;
        let login_id = Uuid::new_v4();
        let cancel = CancellationToken::new();

        {
            let mut guard = self.active_login.lock().await;
            if let Some(existing) = guard.take() {
                drop(existing);
            }
            *guard = Some(ActiveLogin::DeviceCode {
                cancel: cancel.clone(),
                login_id,
            });
        }

        let verification_url = device_code.verification_url.clone();
        let user_code = device_code.user_code.clone();

        let outgoing_clone = self.outgoing.clone();
        let config_manager = self.config_manager.clone();
        let thread_manager = Arc::clone(&self.thread_manager);
        let chatgpt_base_url = self.config.chatgpt_base_url.clone();
        let active_login = self.active_login.clone();
        tokio::spawn(async move {
            let (success, error_msg) = tokio::select! {
                _ = cancel.cancelled() => {
                    (false, Some("Login was not completed".to_string()))
                }
                r = complete_device_code_login(opts, device_code) => {
                    match r {
                        Ok(()) => (true, None),
                        Err(err) => (false, Some(err.to_string())),
                    }
                }
            };

            Self::send_chatgpt_login_completion_notifications(
                &outgoing_clone,
                config_manager,
                thread_manager,
                chatgpt_base_url,
                login_id,
                success,
                error_msg,
            )
            .await;

            let mut guard = active_login.lock().await;
            if guard.as_ref().map(ActiveLogin::login_id) == Some(login_id) {
                *guard = None;
            }
        });

        Ok(LoginAccountResponse::ChatgptDeviceCode {
            login_id: login_id.to_string(),
            verification_url,
            user_code,
        })
    }

    async fn cancel_login_chatgpt_common(
        &self,
        login_id: Uuid,
    ) -> std::result::Result<(), CancelLoginError> {
        let mut guard = self.active_login.lock().await;
        if guard.as_ref().map(ActiveLogin::login_id) == Some(login_id) {
            if let Some(active) = guard.take() {
                drop(active);
            }
            Ok(())
        } else {
            Err(CancelLoginError::NotFound)
        }
    }

    async fn cancel_login_response(
        &self,
        params: CancelLoginAccountParams,
    ) -> Result<CancelLoginAccountResponse, JSONRPCErrorError> {
        let login_id = params.login_id;
        let uuid = Uuid::parse_str(&login_id)
            .map_err(|_| invalid_request(format!("invalid login id: {login_id}")))?;
        let status = match self.cancel_login_chatgpt_common(uuid).await {
            Ok(()) => CancelLoginAccountStatus::Canceled,
            Err(CancelLoginError::NotFound) => CancelLoginAccountStatus::NotFound,
        };
        Ok(CancelLoginAccountResponse { status })
    }

    async fn login_chatgpt_auth_tokens(
        &self,
        request_id: ConnectionRequestId,
        access_token: String,
        chatgpt_account_id: String,
        chatgpt_plan_type: Option<String>,
    ) {
        let result = self
            .login_chatgpt_auth_tokens_response(access_token, chatgpt_account_id, chatgpt_plan_type)
            .await;
        let logged_in = result.is_ok();
        self.outgoing.send_result(request_id, result).await;

        if logged_in {
            self.send_login_success_notifications(/*login_id*/ None)
                .await;
        }
    }

    async fn login_chatgpt_auth_tokens_response(
        &self,
        access_token: String,
        chatgpt_account_id: String,
        chatgpt_plan_type: Option<String>,
    ) -> Result<LoginAccountResponse, JSONRPCErrorError> {
        if matches!(
            self.config.forced_login_method,
            Some(ForcedLoginMethod::Api)
        ) {
            return Err(invalid_request(
                "External ChatGPT auth is disabled. Use API key login instead.",
            ));
        }

        // Cancel any active login attempt to avoid persisting managed auth state.
        {
            let mut guard = self.active_login.lock().await;
            if let Some(active) = guard.take() {
                drop(active);
            }
        }

        if let Some(expected_workspaces) = self.config.forced_chatgpt_workspace_id.as_deref()
            && !expected_workspaces.contains(&chatgpt_account_id)
        {
            return Err(invalid_request(format!(
                "External auth must use one of workspace(s) {expected_workspaces:?}, but received {chatgpt_account_id:?}.",
            )));
        }

        let auth_storage_home = self.auth_storage_home()?;
        login_with_chatgpt_auth_tokens(
            &auth_storage_home,
            &access_token,
            &chatgpt_account_id,
            chatgpt_plan_type.as_deref(),
        )
        .map_err(|err| internal_error(format!("failed to set external auth: {err}")))?;
        self.auth_manager.reload().await;
        self.config_manager.replace_cloud_config_bundle_loader(
            self.auth_manager.clone(),
            self.config.chatgpt_base_url.clone(),
        );
        self.config_manager
            .sync_default_client_residency_requirement()
            .await;

        Ok(LoginAccountResponse::ChatgptAuthTokens {})
    }

    async fn send_login_success_notifications(&self, login_id: Option<Uuid>) {
        Self::maybe_refresh_remote_installed_plugins_cache_for_current_config(
            &self.config_manager,
            &self.thread_manager,
            self.auth_manager.auth_cached(),
        )
        .await;

        let payload_login_completed = AccountLoginCompletedNotification {
            login_id: login_id.map(|id| id.to_string()),
            success: true,
            error: None,
        };
        self.outgoing
            .send_server_notification(ServerNotification::AccountLoginCompleted(
                payload_login_completed,
            ))
            .await;

        self.outgoing
            .send_server_notification(ServerNotification::AccountUpdated(
                self.current_account_updated_notification(),
            ))
            .await;
    }

    async fn send_chatgpt_login_completion_notifications(
        outgoing: &OutgoingMessageSender,
        config_manager: ConfigManager,
        thread_manager: Arc<ThreadManager>,
        chatgpt_base_url: String,
        login_id: Uuid,
        success: bool,
        error_msg: Option<String>,
    ) {
        let payload_v2 = AccountLoginCompletedNotification {
            login_id: Some(login_id.to_string()),
            success,
            error: error_msg,
        };
        outgoing
            .send_server_notification(ServerNotification::AccountLoginCompleted(payload_v2))
            .await;

        if success {
            let auth_manager = thread_manager.auth_manager();
            auth_manager.reload().await;
            config_manager
                .replace_cloud_config_bundle_loader(auth_manager.clone(), chatgpt_base_url);
            config_manager
                .sync_default_client_residency_requirement()
                .await;

            let auth = auth_manager.auth_cached();
            Self::maybe_refresh_remote_installed_plugins_cache_for_current_config(
                &config_manager,
                &thread_manager,
                auth.clone(),
            )
            .await;
            let payload_v2 = AccountUpdatedNotification {
                auth_mode: auth.as_ref().map(CodexAuth::api_auth_mode),
                plan_type: auth.as_ref().and_then(CodexAuth::account_plan_type),
            };
            outgoing
                .send_server_notification(ServerNotification::AccountUpdated(payload_v2))
                .await;
        }
    }

    async fn logout_common(&self) -> std::result::Result<Option<AuthMode>, JSONRPCErrorError> {
        // Cancel any active login attempt.
        {
            let mut guard = self.active_login.lock().await;
            if let Some(active) = guard.take() {
                drop(active);
            }
        }

        match self.auth_manager.logout_with_revoke().await {
            Ok(_) => {}
            Err(err) => {
                return Err(internal_error(format!("logout failed: {err}")));
            }
        }

        Self::maybe_refresh_remote_installed_plugins_cache_for_current_config(
            &self.config_manager,
            &self.thread_manager,
            self.auth_manager.auth_cached(),
        )
        .await;

        // Reflect the current auth method after logout (likely None).
        Ok(self
            .auth_manager
            .auth_cached()
            .as_ref()
            .map(CodexAuth::api_auth_mode))
    }

    async fn logout_v2(&self, request_id: ConnectionRequestId) -> Result<(), JSONRPCErrorError> {
        let result = self.logout_common().await;
        let account_updated =
            result
                .as_ref()
                .ok()
                .cloned()
                .map(|auth_mode| AccountUpdatedNotification {
                    auth_mode,
                    plan_type: None,
                });
        self.outgoing
            .send_result(request_id, result.map(|_| LogoutAccountResponse {}))
            .await;

        if let Some(payload) = account_updated {
            self.outgoing
                .send_server_notification(ServerNotification::AccountUpdated(payload))
                .await;
        }
        Ok(())
    }

    async fn refresh_token_if_requested(&self, do_refresh: bool) -> RefreshTokenRequestOutcome {
        if self.auth_manager.is_external_chatgpt_auth_active() {
            return RefreshTokenRequestOutcome::NotAttemptedOrSucceeded;
        }
        if do_refresh && let Err(err) = self.auth_manager.refresh_token().await {
            let failed_reason = err.failed_reason();
            if failed_reason.is_none() {
                tracing::warn!("failed to refresh token while getting account: {err}");
                return RefreshTokenRequestOutcome::FailedTransiently;
            }
            return RefreshTokenRequestOutcome::FailedPermanently;
        }
        RefreshTokenRequestOutcome::NotAttemptedOrSucceeded
    }

    async fn get_auth_status_response(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<GetAuthStatusResponse, JSONRPCErrorError> {
        let include_token = params.include_token.unwrap_or(false);
        let do_refresh = params.refresh_token.unwrap_or(false);

        self.refresh_token_if_requested(do_refresh).await;

        // Determine whether auth is required based on the active model provider.
        // If a custom provider is configured with `requires_openai_auth == false`,
        // then no auth step is required; otherwise, default to requiring auth.
        let requires_openai_auth = self.config.model_provider.requires_openai_auth;

        let response = if !requires_openai_auth {
            GetAuthStatusResponse {
                auth_method: None,
                auth_token: None,
                requires_openai_auth: Some(false),
            }
        } else {
            let auth = if do_refresh {
                self.auth_manager.auth_cached()
            } else {
                self.auth_manager.auth().await
            };
            match auth {
                Some(auth) => {
                    let permanent_refresh_failure =
                        self.auth_manager.refresh_failure_for_auth(&auth).is_some();
                    let auth_mode = auth.api_auth_mode();
                    let (reported_auth_method, token_opt) = if matches!(
                        auth,
                        CodexAuth::AgentIdentity(_) | CodexAuth::PersonalAccessToken(_)
                    ) || include_token
                        && permanent_refresh_failure
                    {
                        // This response cannot represent the metadata needed to reuse these
                        // credentials.
                        (Some(auth_mode), None)
                    } else {
                        match auth.get_token() {
                            Ok(token) if !token.is_empty() => {
                                let tok = if include_token { Some(token) } else { None };
                                (Some(auth_mode), tok)
                            }
                            Ok(_) => (None, None),
                            Err(err) => {
                                tracing::warn!("failed to get token for auth status: {err}");
                                (None, None)
                            }
                        }
                    };
                    GetAuthStatusResponse {
                        auth_method: reported_auth_method,
                        auth_token: token_opt,
                        requires_openai_auth: Some(true),
                    }
                }
                None => GetAuthStatusResponse {
                    auth_method: None,
                    auth_token: None,
                    requires_openai_auth: Some(true),
                },
            }
        };

        Ok(response)
    }

    async fn get_account_response(
        &self,
        params: GetAccountParams,
    ) -> Result<GetAccountResponse, JSONRPCErrorError> {
        let do_refresh = params.refresh_token;

        self.refresh_token_if_requested(do_refresh).await;

        let provider = create_model_provider(
            self.config.model_provider.clone(),
            Some(self.auth_manager.clone()),
        );
        let account_state = match provider.account_state() {
            Ok(account_state) => account_state,
            Err(err) => return Err(invalid_request(err.to_string())),
        };
        let account = account_state.account.map(Account::from);

        Ok(GetAccountResponse {
            account,
            requires_openai_auth: account_state.requires_openai_auth,
        })
    }

    async fn get_account_rate_limits_response(
        &self,
        params: GetAccountRateLimitsParams,
    ) -> Result<GetAccountRateLimitsResponse, JSONRPCErrorError> {
        self.fetch_account_rate_limits(params).await.map(
            |(
                account_identity_fingerprint,
                rate_limits,
                rate_limits_by_limit_id,
                reset_credits,
            )| GetAccountRateLimitsResponse {
                account_identity_fingerprint,
                rate_limits: rate_limits.into(),
                rate_limits_by_limit_id: Some(
                    rate_limits_by_limit_id
                        .into_iter()
                        .map(|(limit_id, snapshot)| (limit_id, snapshot.into()))
                        .collect(),
                ),
                rate_limit_reset_credits: reset_credits,
            },
        )
    }

    async fn auth_manager_for_rate_limits(
        &self,
        params: GetAccountRateLimitsParams,
    ) -> Result<Arc<AuthManager>, JSONRPCErrorError> {
        self.auth_manager_for_auth_profile(params.auth_profile)
            .await
    }

    async fn auth_manager_for_auth_profile(
        &self,
        auth_profile: Option<Option<String>>,
    ) -> Result<Arc<AuthManager>, JSONRPCErrorError> {
        let Some(auth_profile) = auth_profile else {
            return Ok(self.auth_manager.clone());
        };

        if let Some(profile_name) = auth_profile.as_deref() {
            codex_login::validate_auth_profile_name(profile_name)
                .map_err(|err| invalid_request(format!("invalid auth profile: {err}")))?;
        }

        Ok(self
            .auth_manager
            .shared_scoped_auth_profile(auth_profile)
            .await)
    }

    async fn get_account_token_usage_response(
        &self,
    ) -> Result<GetAccountTokenUsageResponse, JSONRPCErrorError> {
        let Some(auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "codex account authentication required to read token usage",
            ));
        };

        if !auth.uses_codex_backend() {
            return Err(invalid_request(
                "chatgpt authentication required to read token usage",
            ));
        }

        let client = BackendClient::from_auth(self.config.chatgpt_base_url.clone(), &auth)
            .map_err(|err| internal_error(format!("failed to construct backend client: {err}")))?;
        let profile = tokio::time::timeout(
            ACCOUNT_TOKEN_USAGE_FETCH_TIMEOUT,
            client.get_token_usage_profile(),
        )
        .await
        .map_err(|_| internal_error("token usage profile fetch timed out"))?
        .map_err(|err| internal_error(format!("failed to fetch token usage profile: {err}")))?;
        Ok(Self::account_token_usage_response(profile))
    }

    fn account_token_usage_response(profile: TokenUsageProfile) -> GetAccountTokenUsageResponse {
        let stats = profile.stats;
        GetAccountTokenUsageResponse {
            summary: AccountTokenUsageSummary {
                lifetime_tokens: stats.lifetime_tokens,
                peak_daily_tokens: stats.peak_daily_tokens,
                longest_running_turn_sec: stats.longest_running_turn_sec,
                current_streak_days: stats.current_streak_days,
                longest_streak_days: stats.longest_streak_days,
            },
            daily_usage_buckets: stats.daily_usage_buckets.map(|buckets| {
                buckets
                    .into_iter()
                    .map(|bucket| AccountTokenUsageDailyBucket {
                        start_date: bucket.start_date,
                        tokens: bucket.tokens,
                    })
                    .collect()
            }),
        }
    }

    async fn send_add_credits_nudge_email_response(
        &self,
        params: SendAddCreditsNudgeEmailParams,
    ) -> Result<SendAddCreditsNudgeEmailResponse, JSONRPCErrorError> {
        self.send_add_credits_nudge_email_inner(params)
            .await
            .map(|status| SendAddCreditsNudgeEmailResponse { status })
    }

    async fn send_add_credits_nudge_email_inner(
        &self,
        params: SendAddCreditsNudgeEmailParams,
    ) -> Result<AddCreditsNudgeEmailStatus, JSONRPCErrorError> {
        let Some(auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "codex account authentication required to notify workspace owner",
            ));
        };

        if !auth.uses_codex_backend() {
            return Err(invalid_request(
                "chatgpt authentication required to notify workspace owner",
            ));
        }

        let client = BackendClient::from_auth(self.config.chatgpt_base_url.clone(), &auth)
            .map_err(|err| internal_error(format!("failed to construct backend client: {err}")))?;

        match client
            .send_add_credits_nudge_email(Self::backend_credit_type(params.credit_type))
            .await
        {
            Ok(()) => Ok(AddCreditsNudgeEmailStatus::Sent),
            Err(err) if err.status().is_some_and(|status| status.as_u16() == 429) => {
                Ok(AddCreditsNudgeEmailStatus::CooldownActive)
            }
            Err(err) => Err(internal_error(format!(
                "failed to notify workspace owner: {err}"
            ))),
        }
    }

    fn backend_credit_type(value: AddCreditsNudgeCreditType) -> BackendAddCreditsNudgeCreditType {
        match value {
            AddCreditsNudgeCreditType::Credits => BackendAddCreditsNudgeCreditType::Credits,
            AddCreditsNudgeCreditType::UsageLimit => BackendAddCreditsNudgeCreditType::UsageLimit,
        }
    }

    async fn consume_account_rate_limit_reset_credit_response(
        &self,
        params: ConsumeAccountRateLimitResetCreditParams,
    ) -> Result<ConsumeAccountRateLimitResetCreditResponse, JSONRPCErrorError> {
        let ConsumeAccountRateLimitResetCreditParams {
            idempotency_key,
            credit_id,
            auth_profile,
            expected_account_identity_fingerprint,
        } = params;
        let idempotency_key =
            account_rate_limit_resets::validated_idempotency_key(&idempotency_key)?.to_string();
        let credit_id = account_rate_limit_resets::validated_credit_id(credit_id.as_deref())?
            .map(str::to_string);
        let auth_manager = self.auth_manager_for_auth_profile(auth_profile).await?;

        let Some(auth) = auth_manager.auth().await else {
            return Err(invalid_request(
                "codex account authentication required to reset usage limits",
            ));
        };

        if !auth.uses_codex_backend() {
            return Err(invalid_request(
                "chatgpt authentication required to reset usage limits",
            ));
        }

        let account_id = auth.get_account_id().ok_or_else(|| {
            invalid_request("chatgpt account identity required to reset usage limits")
        })?;
        let account_identity_fingerprint = codex_login::account_identity_fingerprint(&account_id);
        if expected_account_identity_fingerprint
            .as_deref()
            .is_some_and(|expected| expected != account_identity_fingerprint)
        {
            return Ok(ConsumeAccountRateLimitResetCreditResponse {
                outcome: ConsumeAccountRateLimitResetCreditOutcome::AccountChanged,
                account_identity_fingerprint,
            });
        }

        let client = BackendClient::from_auth(self.config.chatgpt_base_url.clone(), &auth)
            .map_err(|err| internal_error(format!("failed to construct backend client: {err}")))?;

        let outcome = account_rate_limit_resets::consume_credit(
            &client,
            &idempotency_key,
            credit_id.as_deref(),
        )
        .await?;

        Ok(ConsumeAccountRateLimitResetCreditResponse {
            outcome,
            account_identity_fingerprint,
        })
    }

    async fn fetch_account_rate_limits(
        &self,
        params: GetAccountRateLimitsParams,
    ) -> Result<
        (
            String,
            CoreRateLimitSnapshot,
            HashMap<String, CoreRateLimitSnapshot>,
            Option<RateLimitResetCreditsSummary>,
        ),
        JSONRPCErrorError,
    > {
        let include_reset_credit_details = params.include_reset_credit_details;
        let auth_manager = self.auth_manager_for_rate_limits(params).await?;
        self.fetch_account_rate_limits_with_auth_manager(
            &auth_manager,
            include_reset_credit_details,
        )
        .await
    }

    async fn fetch_account_rate_limits_with_auth_manager(
        &self,
        auth_manager: &AuthManager,
        include_reset_credit_details: bool,
    ) -> Result<
        (
            String,
            CoreRateLimitSnapshot,
            HashMap<String, CoreRateLimitSnapshot>,
            Option<RateLimitResetCreditsSummary>,
        ),
        JSONRPCErrorError,
    > {
        let Some(auth) = auth_manager.auth().await else {
            return Err(invalid_request(
                "codex account authentication required to read rate limits",
            ));
        };

        if !auth.uses_codex_backend() {
            return Err(invalid_request(
                "chatgpt authentication required to read rate limits",
            ));
        }
        let account_id = auth.get_account_id().ok_or_else(|| {
            invalid_request("chatgpt account identity required to read rate limits")
        })?;
        let account_identity_fingerprint = codex_login::account_identity_fingerprint(&account_id);

        let client = BackendClient::from_auth(self.config.chatgpt_base_url.clone(), &auth)
            .map_err(|err| internal_error(format!("failed to construct backend client: {err}")))?;

        let rate_limits = client
            .get_rate_limits_with_reset_credits()
            .await
            .map_err(|err| internal_error(format!("failed to fetch codex rate limits: {err}")))?;
        let reset_credits = account_rate_limit_resets::enrich_summary(
            &client,
            rate_limits.rate_limit_reset_credits,
            include_reset_credit_details,
        )
        .await;
        let snapshots = rate_limits.rate_limits;
        if snapshots.is_empty() {
            return Err(internal_error(
                "failed to fetch codex rate limits: no snapshots returned",
            ));
        }

        let rate_limits_by_limit_id: HashMap<String, CoreRateLimitSnapshot> = snapshots
            .iter()
            .cloned()
            .map(|snapshot| {
                let limit_id = snapshot
                    .limit_id
                    .clone()
                    .unwrap_or_else(|| "codex".to_string());
                (limit_id, snapshot)
            })
            .collect();

        let primary = snapshots
            .iter()
            .find(|snapshot| snapshot.limit_id.as_deref() == Some("codex"))
            .cloned()
            .unwrap_or_else(|| snapshots[0].clone());

        Ok((
            account_identity_fingerprint,
            primary,
            rate_limits_by_limit_id,
            reset_credits,
        ))
    }
}

fn account_auth_storage_home(config: &Config) -> Result<PathBuf, JSONRPCErrorError> {
    let Some(auth_profile_name) = config.selected_auth_profile.as_deref() else {
        return Ok(config.codex_home.to_path_buf());
    };

    ensure_auth_profile_storage_dir(&config.codex_home, auth_profile_name).map_err(|err| {
        invalid_request(format!("selected authentication profile is invalid: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_backend_client::TokenUsageProfileDailyBucket;
    use codex_backend_client::TokenUsageProfileStats;
    use codex_core::config::Config;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[tokio::test]
    async fn account_auth_storage_home_defaults_to_root_auth() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let config = Config::load_default_with_cli_overrides_for_codex_home(
            codex_home.path().to_path_buf(),
            Vec::new(),
        )
        .await?;

        let auth_storage_home =
            account_auth_storage_home(&config).expect("auth storage home should resolve");
        assert_eq!(auth_storage_home, codex_home.path());
        Ok(())
    }

    #[tokio::test]
    async fn account_auth_storage_home_uses_selected_auth_profile() -> anyhow::Result<()> {
        let codex_home = TempDir::new()?;
        let mut config = Config::load_default_with_cli_overrides_for_codex_home(
            codex_home.path().to_path_buf(),
            Vec::new(),
        )
        .await?;
        config.selected_auth_profile = Some("work".to_string());

        let auth_storage_home =
            account_auth_storage_home(&config).expect("auth storage home should resolve");
        assert_eq!(
            auth_storage_home,
            codex_home.path().join("auth_profiles").join("work")
        );
        Ok(())
    }

    #[test]
    fn account_token_usage_response_maps_profile_stats_and_daily_buckets() {
        let response = AccountRequestProcessor::account_token_usage_response(TokenUsageProfile {
            stats: TokenUsageProfileStats {
                lifetime_tokens: Some(123),
                peak_daily_tokens: Some(45),
                longest_running_turn_sec: Some(67),
                current_streak_days: Some(8),
                longest_streak_days: Some(9),
                daily_usage_buckets: Some(vec![TokenUsageProfileDailyBucket {
                    start_date: "2026-05-29".to_string(),
                    tokens: 10,
                }]),
            },
        });

        assert_eq!(
            response,
            GetAccountTokenUsageResponse {
                summary: AccountTokenUsageSummary {
                    lifetime_tokens: Some(123),
                    peak_daily_tokens: Some(45),
                    longest_running_turn_sec: Some(67),
                    current_streak_days: Some(8),
                    longest_streak_days: Some(9),
                },
                daily_usage_buckets: Some(vec![AccountTokenUsageDailyBucket {
                    start_date: "2026-05-29".to_string(),
                    tokens: 10,
                }]),
            }
        );
    }
}
