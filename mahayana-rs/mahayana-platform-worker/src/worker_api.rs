use crate::auth::hash_password_argon2id;
use crate::auth::hash_refresh_token;
use crate::auth::new_password_salt;
use crate::auth::new_refresh_token;
use crate::auth::verify_argon2id;
use crate::auth::verify_pbkdf2_sha256;
use jsonwebtoken::Algorithm;
use jsonwebtoken::DecodingKey;
use jsonwebtoken::EncodingKey;
use jsonwebtoken::Header;
use jsonwebtoken::Validation;
use jsonwebtoken::decode;
use jsonwebtoken::encode;
use mahayana_platform_core::AccountAccessTokenClaims;
use mahayana_platform_core::AccountUsageStatus;
use mahayana_platform_core::Currency;
use mahayana_platform_core::DelegatedTokenRequest;
use mahayana_platform_core::Entitlement;
use mahayana_platform_core::EntitlementStatus;
use mahayana_platform_core::PluginAccessTokenClaims;
use mahayana_platform_core::PurchaseRequest;
use mahayana_platform_core::Quote;
use mahayana_platform_core::UsageCaptureRequest;
use mahayana_platform_core::UsageReservation;
use mahayana_platform_core::UsageReservationRequest;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;
use worker::Context;
use worker::Date;
use worker::Env;
use worker::Method;
use worker::Request;
use worker::Response;
use worker::Result;
use worker::RouteContext;
use worker::Router;
use worker::event;

const DATABASE_BINDING: &str = "PLATFORM_DB";
const ACCOUNT_DATABASE_BINDING: &str = "ACCOUNT_DB";
const ACCESS_TOKEN_ISSUER: &str = "https://api.ombhrum.com";
const ACCESS_TOKEN_AUDIENCE: &str = "mahayana-platform";
const ACCESS_TOKEN_SECONDS: i64 = 15 * 60;
const REFRESH_TOKEN_SECONDS: i64 = 30 * 24 * 60 * 60;
const LOGIN_FAILURE_WINDOW_SECONDS: i64 = 15 * 60;
const MAX_ACCOUNT_LOGIN_FAILURES: i64 = 10;
const USAGE_WINDOW_SECONDS: i64 = 30 * 24 * 60 * 60;
const USAGE_RESERVATION_SECONDS: i64 = 10 * 60;
const MAX_TOKENS_PER_RESERVATION: i64 = 2_000_000;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MarketplacePluginRow {
    plugin_id: String,
    display_name: String,
    description: String,
    latest_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BalanceRow {
    currency: String,
    available: i64,
    reserved: i64,
}

#[derive(Debug, Deserialize)]
struct PriceRow {
    product_id: String,
    price_id: String,
    plugin_id: String,
    sku: String,
    capability: String,
    currency: String,
    amount: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderRow {
    order_id: String,
    plugin_id: String,
    sku: String,
    currency: String,
    amount: i64,
    status: String,
    created_at: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuoteRequest {
    sku: String,
}

#[derive(Debug, Deserialize)]
struct UsageBudgetRow {
    window_start: i64,
    window_end: i64,
    token_limit: i64,
    used_tokens: i64,
    reserved_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct UsageReservationRow {
    reservation_id: String,
    request_id: String,
    reserved_tokens: i64,
    expires_at: i64,
    state: String,
}

#[derive(Debug, Deserialize)]
struct UsageEventRow {
    reservation_id: String,
}

#[derive(Debug, Deserialize)]
struct AccountUserRow {
    id: i64,
    user_no: Option<i64>,
    username: String,
    username_changed_at: Option<String>,
    email: Option<String>,
    nickname: Option<String>,
    avatar: Option<String>,
    phone_number: Option<String>,
    firebase_uid: Option<String>,
    alipay_user_id: Option<String>,
    alipay_nickname: Option<String>,
    alipay_avatar: Option<String>,
    wechat_headimgurl: Option<String>,
    password_hash: Option<String>,
    salt: Option<String>,
    iterations: Option<i64>,
    algo: Option<String>,
    upgraded_password_phc: Option<String>,
    main_practice_title: Option<String>,
    main_practice_file_path: Option<String>,
    main_practice_selected_at: Option<String>,
    created_at: String,
    email_verified: Option<i64>,
    membership_type: Option<String>,
    membership_expires_at: Option<String>,
    free_trial_end_date: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PasswordLoginRequest {
    username: String,
    password: String,
    #[serde(default)]
    device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RefreshAccessRequest {
    refresh_token: String,
    #[serde(default)]
    device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RefreshTokenRow {
    token_hash: String,
    session_id: String,
    generation: i64,
    state: String,
    user_id: String,
    device_id: String,
    session_expires_at: i64,
    revoked_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct LoginFailureCountRow {
    failure_count: i64,
}

#[derive(Debug)]
struct AuthenticatedAccount {
    user_id: String,
    session_id: Option<String>,
}

#[event(fetch, respond_with_errors)]
pub async fn main(request: Request, env: Env, _context: Context) -> Result<Response> {
    Router::new()
        .get("/health", |_, _| Response::from_json(&json!({"ok": true})))
        .post_async("/api/auth/login", password_login)
        .post_async("/api/auth/refresh", refresh_access_token)
        .get_async("/api/auth/user-info", account_user_info)
        .post_async("/api/auth/logout", account_logout)
        .get("/v1/auth/jwks.json", |_, context| {
            let jwks = context.env.secret("ACCESS_TOKEN_JWKS")?.to_string();
            Ok(Response::ok(jwks)?.with_headers(json_headers()))
        })
        .post_async("/v1/auth/plugin-token", delegated_plugin_token)
        .get_async("/v1/ai/usage", ai_usage_status)
        .post_async("/v1/ai/usage/reservations", ai_usage_reserve)
        .post_async(
            "/v1/ai/usage/reservations/:reservation_id/capture",
            ai_usage_capture,
        )
        .post_async(
            "/v1/ai/usage/reservations/:reservation_id/release",
            ai_usage_release,
        )
        .get_async("/v1/marketplace/plugins", marketplace_plugins)
        .get_async("/v1/wallet/balance", wallet_balance)
        .get_async("/v1/wallet/history", wallet_history)
        .post_async("/v1/plugins/:plugin_id/commerce/quote", commerce_quote)
        .post_async(
            "/v1/plugins/:plugin_id/commerce/purchase",
            commerce_purchase,
        )
        .get_async(
            "/v1/plugins/:plugin_id/entitlements/:capability",
            commerce_entitlement,
        )
        .get_async("/v1/purchases", purchases)
        .post_async("/v1/purchases/restore", purchases_restore)
        .run(request, env)
        .await
}

async fn password_login(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    let login: PasswordLoginRequest = match request.json().await {
        Ok(login) => login,
        Err(_) => {
            return error_response(
                400,
                "invalid_login_request",
                "用户名或邮箱、手机号和密码不能为空",
            );
        }
    };
    let identifier = login.username.trim();
    if identifier.is_empty() || login.password.is_empty() || login.password.len() > 1024 {
        return error_response(
            400,
            "invalid_login_request",
            "用户名或邮箱、手机号和密码不能为空",
        );
    }
    let device_id = match normalize_device_id(login.device_id.as_deref()) {
        Ok(device_id) => device_id,
        Err(_) => return error_response(400, "invalid_device_id", "invalid device id"),
    };
    let database = context.env.d1(ACCOUNT_DATABASE_BINDING)?;
    let user = lookup_login_user(&database, identifier).await?;
    let Some(user) = user else {
        return error_response(401, "invalid_credentials", "账号或密码错误");
    };
    let now = now_seconds();
    if account_login_is_rate_limited(&database, &user.id.to_string(), now).await? {
        return error_response(429, "login_rate_limited", "登录尝试过多，请稍后再试");
    }

    let password_valid = if let Some(upgraded) = user.upgraded_password_phc.as_deref() {
        verify_argon2id(&login.password, upgraded)
    } else {
        let Some(password_hash) = user.password_hash.as_deref() else {
            return error_response(401, "password_not_configured", "当前账号尚未设置密码");
        };
        let Some(salt) = user.salt.as_deref() else {
            return error_response(401, "password_not_configured", "当前账号尚未设置密码");
        };
        verify_pbkdf2_sha256(
            &login.password,
            salt,
            password_hash,
            user.iterations,
            user.algo.as_deref(),
        )
        .unwrap_or(false)
    };
    if !password_valid {
        record_auth_event(
            &database,
            Some(&user.id.to_string()),
            None,
            "login_failed",
            now_seconds(),
        )
        .await?;
        return error_response(401, "invalid_credentials", "账号或密码错误");
    }

    if user.upgraded_password_phc.is_none() {
        let upgraded = hash_password_argon2id(&login.password, &new_password_salt())
            .map_err(|error| worker::Error::RustError(error.to_string()))?;
        worker::query!(
            &database,
            "INSERT OR IGNORE INTO account_password_credentials
             (user_id, password_phc, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)",
            user.id.to_string(),
            upgraded,
            now
        )?
        .run()
        .await?;
    }

    let session_id = Uuid::new_v4().to_string();
    let family_id = Uuid::new_v4().to_string();
    let refresh_token = new_refresh_token();
    let refresh_hash = hash_refresh_token(&refresh_token);
    let refresh_expires_at = now + REFRESH_TOKEN_SECONDS;
    let (access_token, access_expires_at, access_jti) = issue_account_access_token(
        &context.env,
        &user.id.to_string(),
        &device_id,
        &session_id,
        now,
    )?;
    let statements = vec![
        worker::query!(
            &database,
            "INSERT INTO account_sessions
             (session_id, refresh_family_id, user_id, device_id, current_refresh_token_hash,
              created_at, last_used_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
            &session_id,
            &family_id,
            user.id.to_string(),
            &device_id,
            &refresh_hash,
            now,
            refresh_expires_at
        )?,
        worker::query!(
            &database,
            "INSERT INTO account_refresh_tokens
             (token_hash, session_id, generation, state, issued_at, expires_at)
             VALUES (?1, ?2, 0, 'active', ?3, ?4)",
            &refresh_hash,
            &session_id,
            now,
            refresh_expires_at
        )?,
        worker::query!(
            &database,
            "INSERT INTO account_auth_events
             (event_id, user_id, session_id, event_type, occurred_at, details_json)
             VALUES (?1, ?2, ?3, 'login_succeeded', ?4, ?5)",
            Uuid::new_v4().to_string(),
            user.id.to_string(),
            &session_id,
            now,
            json!({"accessJti": access_jti}).to_string()
        )?,
    ];
    database.batch(statements).await?;

    account_session_response(
        &user,
        &access_token,
        &refresh_token,
        access_expires_at,
        refresh_expires_at,
        &session_id,
        &device_id,
    )
}

async fn refresh_access_token(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    let refresh: RefreshAccessRequest = match request.json().await {
        Ok(refresh) => refresh,
        Err(_) => {
            return error_response(400, "invalid_refresh_request", "refresh token is required");
        }
    };
    if !refresh.refresh_token.starts_with("mrt_") || refresh.refresh_token.len() != 68 {
        return error_response(401, "invalid_refresh_token", "登录会话已失效");
    }
    let database = context.env.d1(ACCOUNT_DATABASE_BINDING)?;
    let token_hash = hash_refresh_token(&refresh.refresh_token);
    let row = worker::query!(
        &database,
        "SELECT rt.token_hash, rt.session_id, rt.generation, rt.state,
                s.user_id, s.device_id, s.expires_at AS session_expires_at, s.revoked_at
         FROM account_refresh_tokens rt
         JOIN account_sessions s ON s.session_id = rt.session_id
         WHERE rt.token_hash = ?1
         LIMIT 1",
        &token_hash
    )?
    .first::<RefreshTokenRow>(None)
    .await?;
    let Some(row) = row else {
        return error_response(401, "invalid_refresh_token", "登录会话已失效");
    };
    let now = now_seconds();
    if row.state != "active" {
        revoke_account_session(&database, &row.session_id, "refresh_token_reuse", now).await?;
        return error_response(401, "refresh_token_reused", "登录会话已撤销，请重新登录");
    }
    if row.revoked_at.is_some() || row.session_expires_at <= now {
        return error_response(401, "refresh_token_expired", "登录会话已过期，请重新登录");
    }
    if let Some(device_id) = refresh.device_id.as_deref()
        && device_id != row.device_id
    {
        return error_response(401, "device_mismatch", "登录设备不匹配，请重新登录");
    }
    let user = lookup_account_user_by_id(&database, &row.user_id).await?;
    let Some(user) = user else {
        revoke_account_session(&database, &row.session_id, "account_missing", now).await?;
        return error_response(401, "account_missing", "账号不存在");
    };

    let next_refresh = new_refresh_token();
    let next_hash = hash_refresh_token(&next_refresh);
    let next_generation = row.generation + 1;
    let (access_token, access_expires_at, access_jti) = issue_account_access_token(
        &context.env,
        &row.user_id,
        &row.device_id,
        &row.session_id,
        now,
    )?;
    let statements = vec![
        worker::query!(
            &database,
            "UPDATE account_refresh_tokens
             SET state = 'used', used_at = ?1, replaced_by_hash = ?2
             WHERE token_hash = ?3 AND state = 'active'",
            now,
            &next_hash,
            &row.token_hash
        )?,
        worker::query!(
            &database,
            "INSERT INTO account_refresh_tokens
             (token_hash, session_id, generation, state, issued_at, expires_at)
             VALUES (?1, ?2, ?3, 'active', ?4, ?5)",
            &next_hash,
            &row.session_id,
            next_generation,
            now,
            row.session_expires_at
        )?,
        worker::query!(
            &database,
            "UPDATE account_sessions
             SET current_refresh_token_hash = ?1, last_used_at = ?2
             WHERE session_id = ?3 AND revoked_at IS NULL",
            &next_hash,
            now,
            &row.session_id
        )?,
        worker::query!(
            &database,
            "INSERT INTO account_auth_events
             (event_id, user_id, session_id, event_type, occurred_at, details_json)
             VALUES (?1, ?2, ?3, 'refresh_rotated', ?4, ?5)",
            Uuid::new_v4().to_string(),
            &row.user_id,
            &row.session_id,
            now,
            json!({"generation": next_generation, "accessJti": access_jti}).to_string()
        )?,
    ];
    if database.batch(statements).await.is_err() {
        return error_response(
            409,
            "refresh_conflict",
            "登录会话正在轮换，请使用最新凭据重试",
        );
    }

    account_session_response(
        &user,
        &access_token,
        &next_refresh,
        access_expires_at,
        row.session_expires_at,
        &row.session_id,
        &row.device_id,
    )
}

async fn account_user_info(request: Request, context: RouteContext<()>) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "登录已过期，请重新登录"),
    };
    let database = context.env.d1(ACCOUNT_DATABASE_BINDING)?;
    let Some(user) = lookup_account_user_by_id(&database, &account.user_id).await? else {
        return error_response(404, "account_missing", "账号不存在");
    };
    Ok(Response::from_json(&serialize_account_user(&user))?.with_headers(auth_headers()))
}

async fn account_logout(request: Request, context: RouteContext<()>) -> Result<Response> {
    let account = match authenticated_account(&request, &context.env) {
        Ok(account) => account,
        Err(_) => return error_response(401, "unauthorized", "登录已过期，请重新登录"),
    };
    if let Some(session_id) = account.session_id {
        let database = context.env.d1(ACCOUNT_DATABASE_BINDING)?;
        revoke_account_session(&database, &session_id, "logout", now_seconds()).await?;
    }
    Ok(
        Response::from_json(&json!({"success": true, "loggedIn": false}))?
            .with_headers(auth_headers()),
    )
}

async fn ai_usage_status(request: Request, context: RouteContext<()>) -> Result<Response> {
    let user_id = authenticated_user(&request, &context.env)?;
    let database = context.env.d1(DATABASE_BINDING)?;
    let status = current_usage_status(&database, &context.env, &user_id, now_seconds()).await?;
    Response::from_json(&status)
}

async fn ai_usage_reserve(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    require_model_gateway(&request, &context.env)?;
    let user_id = authenticated_user(&request, &context.env)?;
    let reservation: UsageReservationRequest = request.json().await?;
    if !is_opaque_id(&reservation.request_id)
        || reservation.input_token_budget < 0
        || reservation.output_token_budget < 0
    {
        return error_response(
            400,
            "invalid_usage_reservation",
            "invalid usage reservation",
        );
    }
    let reserved_tokens = reservation
        .input_token_budget
        .checked_add(reservation.output_token_budget)
        .filter(|tokens| *tokens > 0 && *tokens <= MAX_TOKENS_PER_RESERVATION)
        .ok_or_else(|| worker::Error::RustError("invalid token reservation size".into()))?;
    let database = context.env.d1(DATABASE_BINDING)?;
    let now = now_seconds();
    expire_usage_reservations(&database, &user_id, now).await?;
    if let Some(existing) =
        usage_reservation_by_request(&database, &user_id, &reservation.request_id).await?
    {
        return Response::from_json(&UsageReservation {
            reservation_id: existing.reservation_id,
            request_id: existing.request_id,
            reserved_tokens: existing.reserved_tokens,
            expires_at: existing.expires_at,
        });
    }

    let window_start = usage_window_start(now);
    let window_end = window_start + USAGE_WINDOW_SECONDS;
    let default_limit = default_usage_limit(&context.env)?;
    worker::query!(
        &database,
        "INSERT OR IGNORE INTO ai_usage_budgets
         (user_id, window_start, window_end, token_limit, used_tokens, reserved_tokens, updated_at)
         VALUES (?1, ?2, ?3, ?4, 0, 0, ?5)",
        &user_id,
        window_start,
        window_end,
        default_limit,
        now
    )?
    .run()
    .await?;

    let reservation_id = Uuid::new_v4().to_string();
    let expires_at = now + USAGE_RESERVATION_SECONDS;
    let results = database
        .batch(vec![
            worker::query!(
                &database,
                "INSERT OR IGNORE INTO ai_usage_reservations
                 (reservation_id, user_id, window_start, request_id, input_token_budget,
                  output_token_budget, reserved_tokens, state, expires_at, created_at, updated_at)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'reserved', ?8, ?9, ?9
                 FROM ai_usage_budgets b
                 WHERE b.user_id = ?2 AND b.window_start = ?3
                   AND b.token_limit - b.used_tokens - b.reserved_tokens >= ?7",
                &reservation_id,
                &user_id,
                window_start,
                &reservation.request_id,
                reservation.input_token_budget,
                reservation.output_token_budget,
                reserved_tokens,
                expires_at,
                now
            )?,
            worker::query!(
                &database,
                "UPDATE ai_usage_budgets
                 SET reserved_tokens = reserved_tokens + ?1, updated_at = ?2
                 WHERE user_id = ?3 AND window_start = ?4
                   AND EXISTS (
                       SELECT 1 FROM ai_usage_reservations r
                       WHERE r.reservation_id = ?5 AND r.user_id = ?3
                         AND r.window_start = ?4 AND r.state = 'reserved'
                   )",
                reserved_tokens,
                now,
                &user_id,
                window_start,
                &reservation_id
            )?,
        ])
        .await?;
    if d1_changes(results.first()) == 0 {
        if let Some(existing) =
            usage_reservation_by_request(&database, &user_id, &reservation.request_id).await?
        {
            return Response::from_json(&UsageReservation {
                reservation_id: existing.reservation_id,
                request_id: existing.request_id,
                reserved_tokens: existing.reserved_tokens,
                expires_at: existing.expires_at,
            });
        }
        let status = current_usage_status(&database, &context.env, &user_id, now).await?;
        return usage_limit_response(&status);
    }
    Response::from_json(&UsageReservation {
        reservation_id,
        request_id: reservation.request_id,
        reserved_tokens,
        expires_at,
    })
}

async fn ai_usage_capture(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    require_model_gateway(&request, &context.env)?;
    let user_id = authenticated_user(&request, &context.env)?;
    let reservation_id = route_identifier(&context, "reservation_id")?;
    let capture: UsageCaptureRequest = request.json().await?;
    if !is_opaque_id(&capture.provider_response_id)
        || [
            capture.input_tokens,
            capture.cached_input_tokens,
            capture.output_tokens,
            capture.reasoning_output_tokens,
            capture.total_tokens,
        ]
        .into_iter()
        .any(|tokens| tokens < 0)
        || capture.cached_input_tokens > capture.input_tokens
        || capture.reasoning_output_tokens > capture.output_tokens
        || capture.total_tokens != capture.input_tokens.saturating_add(capture.output_tokens)
    {
        return error_response(
            400,
            "invalid_usage_capture",
            "invalid provider usage breakdown",
        );
    }
    let database = context.env.d1(DATABASE_BINDING)?;
    if let Some(existing) =
        usage_event_by_response(&database, &capture.provider_response_id).await?
    {
        if existing.reservation_id != reservation_id {
            return error_response(
                409,
                "usage_response_conflict",
                "provider response was already captured",
            );
        }
        let status = current_usage_status(&database, &context.env, &user_id, now_seconds()).await?;
        return Response::from_json(&status);
    }
    let Some(reservation) = usage_reservation_by_id(&database, &user_id, reservation_id).await?
    else {
        return error_response(
            404,
            "usage_reservation_not_found",
            "usage reservation was not found",
        );
    };
    if reservation.state != "reserved" {
        return error_response(
            409,
            "usage_reservation_terminal",
            "usage reservation is already terminal",
        );
    }
    if capture.total_tokens > reservation.reserved_tokens {
        return error_response(
            409,
            "usage_capture_exceeds_reservation",
            "provider usage exceeds reservation",
        );
    }
    let now = now_seconds();
    let event_id = Uuid::new_v4().to_string();
    let results = database
        .batch(vec![
            worker::query!(
                &database,
                "INSERT INTO ai_usage_events
                 (event_id, reservation_id, provider_response_id, input_tokens,
                  cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens, created_at)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
                 FROM ai_usage_reservations r
                 WHERE r.reservation_id = ?2 AND r.user_id = ?10 AND r.state = 'reserved'
                   AND ?8 <= r.reserved_tokens
                   AND EXISTS (
                       SELECT 1 FROM ai_usage_budgets b
                       WHERE b.user_id = r.user_id AND b.window_start = r.window_start
                   )",
                &event_id,
                reservation_id,
                &capture.provider_response_id,
                capture.input_tokens,
                capture.cached_input_tokens,
                capture.output_tokens,
                capture.reasoning_output_tokens,
                capture.total_tokens,
                now,
                &user_id
            )?,
            worker::query!(
                &database,
                "UPDATE ai_usage_budgets
                 SET reserved_tokens = reserved_tokens - (
                         SELECT r.reserved_tokens FROM ai_usage_reservations r
                         WHERE r.reservation_id = ?1 AND r.user_id = ?2
                     ),
                     used_tokens = used_tokens + ?3,
                     updated_at = ?4
                 WHERE user_id = ?2
                   AND window_start = (
                       SELECT r.window_start FROM ai_usage_reservations r
                       WHERE r.reservation_id = ?1 AND r.user_id = ?2
                   )
                   AND EXISTS (
                       SELECT 1 FROM ai_usage_events e WHERE e.event_id = ?5
                   )",
                reservation_id,
                &user_id,
                capture.total_tokens,
                now,
                &event_id
            )?,
            worker::query!(
                &database,
                "UPDATE ai_usage_reservations
                 SET actual_input_tokens = ?1, actual_cached_input_tokens = ?2,
                     actual_output_tokens = ?3, actual_reasoning_output_tokens = ?4,
                     actual_total_tokens = ?5, state = 'captured', updated_at = ?6
                 WHERE reservation_id = ?7 AND user_id = ?8 AND state = 'reserved'
                   AND EXISTS (
                       SELECT 1 FROM ai_usage_events e WHERE e.event_id = ?9
                   )",
                capture.input_tokens,
                capture.cached_input_tokens,
                capture.output_tokens,
                capture.reasoning_output_tokens,
                capture.total_tokens,
                now,
                reservation_id,
                &user_id,
                &event_id
            )?,
        ])
        .await?;
    if d1_changes(results.first()) == 0 {
        if let Some(existing) =
            usage_event_by_response(&database, &capture.provider_response_id).await?
        {
            if existing.reservation_id != reservation_id {
                return error_response(
                    409,
                    "usage_response_conflict",
                    "provider response was already captured",
                );
            }
            let status = current_usage_status(&database, &context.env, &user_id, now).await?;
            return Response::from_json(&status);
        }
        return error_response(
            409,
            "usage_reservation_terminal",
            "usage reservation is already terminal",
        );
    }
    let status = current_usage_status(&database, &context.env, &user_id, now).await?;
    Response::from_json(&status)
}

async fn ai_usage_release(request: Request, context: RouteContext<()>) -> Result<Response> {
    require_model_gateway(&request, &context.env)?;
    let user_id = authenticated_user(&request, &context.env)?;
    let reservation_id = route_identifier(&context, "reservation_id")?;
    let database = context.env.d1(DATABASE_BINDING)?;
    let now = now_seconds();
    database
        .batch(vec![
            worker::query!(
                &database,
                "UPDATE ai_usage_budgets
                 SET reserved_tokens = reserved_tokens - (
                         SELECT r.reserved_tokens FROM ai_usage_reservations r
                         WHERE r.reservation_id = ?1 AND r.user_id = ?2 AND r.state = 'reserved'
                     ),
                     updated_at = ?3
                 WHERE user_id = ?2
                   AND window_start = (
                       SELECT r.window_start FROM ai_usage_reservations r
                       WHERE r.reservation_id = ?1 AND r.user_id = ?2 AND r.state = 'reserved'
                   )",
                reservation_id,
                &user_id,
                now
            )?,
            worker::query!(
                &database,
                "UPDATE ai_usage_reservations SET state = 'released', updated_at = ?1
                 WHERE reservation_id = ?2 AND user_id = ?3 AND state = 'reserved'
                   AND EXISTS (
                       SELECT 1 FROM ai_usage_budgets b
                       WHERE b.user_id = ?3 AND b.window_start = ai_usage_reservations.window_start
                   )",
                now,
                reservation_id,
                &user_id
            )?,
        ])
        .await?;
    let status = current_usage_status(&database, &context.env, &user_id, now).await?;
    Response::from_json(&status)
}

async fn marketplace_plugins(request: Request, context: RouteContext<()>) -> Result<Response> {
    let query = request.url()?.query_pairs().find_map(|(key, value)| {
        (key == "q" && !value.trim().is_empty()).then(|| format!("%{}%", value.trim()))
    });
    let database = context.env.d1(DATABASE_BINDING)?;
    let rows = if let Some(query) = query {
        worker::query!(
            &database,
            "SELECT plugin_id, display_name, description, latest_version
             FROM marketplace_plugins
             WHERE visibility = 'public' AND review_state = 'approved'
               AND (display_name LIKE ?1 OR description LIKE ?1 OR plugin_id LIKE ?1)
             ORDER BY updated_at DESC LIMIT 100",
            &query
        )?
        .all()
        .await?
        .results::<MarketplacePluginRow>()?
    } else {
        database
            .prepare(
                "SELECT plugin_id, display_name, description, latest_version
                 FROM marketplace_plugins
                 WHERE visibility = 'public' AND review_state = 'approved'
                 ORDER BY updated_at DESC LIMIT 100",
            )
            .all()
            .await?
            .results::<MarketplacePluginRow>()?
    };
    Response::from_json(&json!({"plugins": rows}))
}

async fn wallet_balance(request: Request, context: RouteContext<()>) -> Result<Response> {
    let user_id = authenticated_user(&request, &context.env)?;
    let database = context.env.d1(DATABASE_BINDING)?;
    let row = worker::query!(
        &database,
        "SELECT
             wb.currency AS currency,
             wb.balance - COALESCE((
                 SELECT SUM(cr.amount) FROM consumption_reservations cr
                 WHERE cr.user_id = ?1 AND cr.currency = wb.currency AND cr.state = 'reserved'
             ), 0) AS available,
             COALESCE((
                 SELECT SUM(cr.amount) FROM consumption_reservations cr
                 WHERE cr.user_id = ?1 AND cr.currency = wb.currency AND cr.state = 'reserved'
             ), 0) AS reserved
         FROM wallet_balances wb
         WHERE wb.owner_type = 'user' AND wb.owner_id = ?1 AND wb.currency = 'MBC'",
        &user_id
    )?
    .first::<BalanceRow>(None)
    .await?
    .unwrap_or(BalanceRow {
        currency: "MBC".into(),
        available: 0,
        reserved: 0,
    });
    Response::from_json(&json!({
        "currency": Currency(row.currency),
        "available": row.available,
        "reserved": row.reserved,
    }))
}

async fn wallet_history(request: Request, context: RouteContext<()>) -> Result<Response> {
    let user_id = authenticated_user(&request, &context.env)?;
    let database = context.env.d1(DATABASE_BINDING)?;
    let account_id = format!("user:{user_id}:MBC");
    let rows = worker::query!(
        &database,
        "SELECT je.entry_id, je.reference_type, je.reference_id, je.created_at, jl.amount, jl.currency
         FROM journal_lines jl
         JOIN journal_entries je ON je.entry_id = jl.entry_id
         WHERE jl.account_id = ?1 AND je.state = 'posted'
         ORDER BY je.created_at DESC LIMIT 100",
        &account_id
    )?
    .all()
    .await?
    .results::<serde_json::Value>()?;
    Response::from_json(&json!({"entries": rows, "nextCursor": null}))
}

async fn commerce_quote(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    let _user_id = authenticated_user(&request, &context.env)?;
    let plugin_id = route_identifier(&context, "plugin_id")?;
    let quote_request: QuoteRequest = request.json().await?;
    let database = context.env.d1(DATABASE_BINDING)?;
    let now = now_seconds();
    let price = active_price(&database, plugin_id, quote_request.sku.trim(), now).await?;
    let Some(price) = price else {
        return error_response(404, "product_not_found", "SKU is not available");
    };
    Response::from_json(&Quote {
        quote_id: Uuid::new_v4().to_string(),
        plugin_id: price.plugin_id,
        sku: price.sku,
        amount: price.amount,
        currency: Currency(price.currency),
        expires_at: now + 300,
    })
}

async fn commerce_purchase(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    let user_id = authenticated_user(&request, &context.env)?;
    let plugin_id = route_identifier(&context, "plugin_id")?;
    let purchase: PurchaseRequest = request.json().await?;
    let database = context.env.d1(DATABASE_BINDING)?;
    let now = now_seconds();
    let Some(price) = active_price(&database, plugin_id, purchase.sku.trim(), now).await? else {
        return error_response(404, "product_not_found", "SKU is not available");
    };
    if let Some(existing) =
        order_by_idempotency(&database, &user_id, &purchase.idempotency_key).await?
    {
        if existing.plugin_id != price.plugin_id
            || existing.sku != price.sku
            || existing.currency != price.currency
            || existing.amount != price.amount
        {
            return error_response(
                409,
                "idempotency_conflict",
                "idempotency key was already used for a different product or price",
            );
        }
        if existing.status == "fulfilled" {
            let entitlement = entitlement_for_order(&database, &existing.order_id).await?;
            return Response::from_json(&json!({
                "orderId": existing.order_id,
                "status": existing.status,
                "entitlement": entitlement,
            }));
        }
    }
    let order_id = Uuid::new_v4().to_string();
    let entry_id = Uuid::new_v4().to_string();
    let entitlement_id = Uuid::new_v4().to_string();
    let user_account = format!("user:{user_id}:{}", price.currency);
    let platform_account = format!("platform:content:{}", price.currency);
    let user_line_id = Uuid::new_v4().to_string();
    let platform_line_id = Uuid::new_v4().to_string();
    let audit_id = Uuid::new_v4().to_string();
    let statements = vec![
        worker::query!(
            &database,
            "INSERT OR IGNORE INTO wallet_accounts
             (account_id, owner_type, owner_id, currency, created_at)
             VALUES (?1, 'user', ?2, ?3, ?4)",
            &user_account,
            &user_id,
            &price.currency,
            now
        )?,
        worker::query!(
            &database,
            "INSERT OR IGNORE INTO wallet_accounts
             (account_id, owner_type, owner_id, currency, created_at)
             VALUES (?1, 'platform', 'digital-content', ?2, ?3)",
            &platform_account,
            &price.currency,
            now
        )?,
        worker::query!(
            &database,
            "INSERT INTO orders
             (order_id, buyer_user_id, plugin_id, product_id, price_id, sku, currency, amount,
              status, idempotency_key, created_at, updated_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?10
             WHERE COALESCE((SELECT balance FROM wallet_balances WHERE account_id = ?11), 0) >= ?8
             ON CONFLICT(buyer_user_id, idempotency_key) DO NOTHING",
            &order_id,
            &user_id,
            &price.plugin_id,
            &price.product_id,
            &price.price_id,
            &price.sku,
            &price.currency,
            price.amount,
            &purchase.idempotency_key,
            now,
            &user_account
        )?,
        worker::query!(
            &database,
            "INSERT OR IGNORE INTO journal_entries
             (entry_id, reference_type, reference_id, state, created_at)
             SELECT ?1, 'order', order_id, 'draft', ?2 FROM orders
             WHERE buyer_user_id = ?3 AND idempotency_key = ?4",
            &entry_id,
            now,
            &user_id,
            &purchase.idempotency_key
        )?,
        worker::query!(
            &database,
            "INSERT OR IGNORE INTO journal_lines
             (line_id, entry_id, account_id, currency, amount, created_at)
             SELECT ?1, je.entry_id, ?2, ?3, ?4, ?5 FROM journal_entries je
             JOIN orders o ON o.order_id = je.reference_id
             WHERE je.reference_type = 'order' AND je.state = 'draft'
               AND o.buyer_user_id = ?6 AND o.idempotency_key = ?7",
            &user_line_id,
            &user_account,
            &price.currency,
            -price.amount,
            now,
            &user_id,
            &purchase.idempotency_key
        )?,
        worker::query!(
            &database,
            "INSERT OR IGNORE INTO journal_lines
             (line_id, entry_id, account_id, currency, amount, created_at)
             SELECT ?1, je.entry_id, ?2, ?3, ?4, ?5 FROM journal_entries je
             JOIN orders o ON o.order_id = je.reference_id
             WHERE je.reference_type = 'order' AND je.state = 'draft'
               AND o.buyer_user_id = ?6 AND o.idempotency_key = ?7",
            &platform_line_id,
            &platform_account,
            &price.currency,
            price.amount,
            now,
            &user_id,
            &purchase.idempotency_key
        )?,
        worker::query!(
            &database,
            "UPDATE journal_entries SET state = 'posted', posted_at = ?1
             WHERE entry_id IN (
                 SELECT je.entry_id FROM journal_entries je JOIN orders o ON o.order_id = je.reference_id
                 WHERE je.reference_type = 'order' AND o.buyer_user_id = ?2 AND o.idempotency_key = ?3
             ) AND state = 'draft'
               AND (SELECT COUNT(*) FROM journal_lines jl WHERE jl.entry_id = journal_entries.entry_id) >= 2
               AND NOT EXISTS (
                   SELECT currency FROM journal_lines jl
                   WHERE jl.entry_id = journal_entries.entry_id
                   GROUP BY currency HAVING SUM(amount) <> 0
               )",
            now,
            &user_id,
            &purchase.idempotency_key
        )?,
        worker::query!(
            &database,
            "INSERT OR IGNORE INTO entitlements
             (entitlement_id, user_id, plugin_id, product_id, order_id, capability, status, granted_at)
             SELECT ?1, ?2, o.plugin_id, o.product_id, o.order_id, ?3, 'active', ?4
             FROM orders o JOIN journal_entries je
               ON je.reference_type = 'order' AND je.reference_id = o.order_id AND je.state = 'posted'
             WHERE o.buyer_user_id = ?2 AND o.idempotency_key = ?5",
            &entitlement_id,
            &user_id,
            &price.capability,
            now,
            &purchase.idempotency_key
        )?,
        worker::query!(
            &database,
            "UPDATE orders SET status = 'fulfilled', updated_at = ?1
             WHERE buyer_user_id = ?2 AND idempotency_key = ?3 AND status = 'pending'
               AND EXISTS (
                   SELECT 1 FROM entitlements e
                   WHERE e.order_id = orders.order_id AND e.status = 'active'
               )",
            now,
            &user_id,
            &purchase.idempotency_key
        )?,
        worker::query!(
            &database,
            "INSERT OR IGNORE INTO audit_events
             (event_id, actor_type, actor_id, event_type, subject_type, subject_id, payload_json, created_at)
             SELECT ?1, 'user', ?2, 'commerce.purchase', 'order', o.order_id, '{}', ?3
             FROM orders o
             WHERE o.buyer_user_id = ?2 AND o.idempotency_key = ?4 AND o.status = 'fulfilled'",
            &audit_id,
            &user_id,
            now,
            &purchase.idempotency_key
        )?,
    ];
    database.batch(statements).await?;
    let order = order_by_idempotency(&database, &user_id, &purchase.idempotency_key).await?;
    let Some(order) = order else {
        return error_response(
            402,
            "insufficient_balance",
            "insufficient Mahayana bean balance",
        );
    };
    if order.plugin_id != price.plugin_id || order.sku != price.sku {
        return error_response(
            409,
            "idempotency_conflict",
            "idempotency key was already used for a different product",
        );
    }
    if order.status != "fulfilled" {
        return error_response(
            500,
            "ledger_invariant_violation",
            "order could not be posted as a balanced journal entry",
        );
    }
    let entitlement = entitlement_for_order(&database, &order.order_id).await?;
    Response::from_json(&json!({
        "orderId": order.order_id,
        "status": order.status,
        "entitlement": entitlement,
    }))
}

async fn commerce_entitlement(request: Request, context: RouteContext<()>) -> Result<Response> {
    let user_id = authenticated_user(&request, &context.env)?;
    let plugin_id = route_identifier(&context, "plugin_id")?;
    let capability = route_identifier(&context, "capability")?;
    let database = context.env.d1(DATABASE_BINDING)?;
    #[derive(Deserialize)]
    struct EntitlementRow {
        entitlement_id: String,
        user_id: String,
        plugin_id: String,
        capability: String,
        expires_at: Option<i64>,
    }
    let row = worker::query!(
        &database,
        "SELECT entitlement_id, user_id, plugin_id, capability, expires_at
         FROM entitlements
         WHERE user_id = ?1 AND plugin_id = ?2 AND capability = ?3 AND status = 'active'
           AND (expires_at IS NULL OR expires_at > ?4)
         ORDER BY granted_at DESC LIMIT 1",
        &user_id,
        plugin_id,
        capability,
        now_seconds()
    )?
    .first::<EntitlementRow>(None)
    .await?;
    let entitlement = row.map(|row| Entitlement {
        entitlement_id: row.entitlement_id,
        user_id: row.user_id,
        plugin_id: row.plugin_id,
        capability: row.capability,
        status: EntitlementStatus::Active,
        expires_at: row.expires_at,
    });
    Response::from_json(&json!({"entitlement": entitlement}))
}

async fn delegated_plugin_token(
    mut request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let user_id = authenticated_user(&request, &context.env)?;
    let delegated: DelegatedTokenRequest = request.json().await?;
    validate_delegated_request(&delegated)?;
    let now = now_seconds() as usize;
    let expires_at = now + 300;
    let claims = PluginAccessTokenClaims {
        iss: ACCESS_TOKEN_ISSUER.to_string(),
        sub: user_id,
        aud: format!("plugin:{}", delegated.plugin_id),
        scope: delegated.scopes,
        device_id: delegated.device_id,
        jti: Uuid::new_v4().to_string(),
        iat: now,
        exp: expires_at,
        token_use: "plugin".to_string(),
    };
    let private_key = context
        .env
        .secret("PLUGIN_TOKEN_PRIVATE_KEY_PEM")?
        .to_string();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = context
        .env
        .var("PLUGIN_TOKEN_KEY_ID")
        .ok()
        .map(|value| value.to_string());
    let key = EncodingKey::from_rsa_pem(private_key.as_bytes()).map_err(jwt_error)?;
    let token = encode(&header, &claims, &key).map_err(jwt_error)?;
    Response::from_json(&json!({
        "accessToken": token,
        "tokenType": "Bearer",
        "expiresIn": 300,
        "expiresAt": expires_at,
    }))
}

fn validate_delegated_request(request: &DelegatedTokenRequest) -> Result<()> {
    if !is_identifier(&request.plugin_id) {
        return Err(worker::Error::RustError(
            "invalid delegated plugin id".into(),
        ));
    }
    if request.device_id.trim().is_empty() || request.device_id.len() > 128 {
        return Err(worker::Error::RustError(
            "invalid delegated device id".into(),
        ));
    }
    if request.scopes.len() > 32
        || request
            .scopes
            .iter()
            .any(|scope| scope.len() > 96 || !is_scope(scope))
    {
        return Err(worker::Error::RustError(
            "invalid delegated token scopes".into(),
        ));
    }
    Ok(())
}

async fn purchases(request: Request, context: RouteContext<()>) -> Result<Response> {
    let user_id = authenticated_user(&request, &context.env)?;
    purchases_response(&context.env, &user_id).await
}

async fn purchases_restore(request: Request, context: RouteContext<()>) -> Result<Response> {
    if request.method() != Method::Post {
        return error_response(405, "method_not_allowed", "POST required");
    }
    let user_id = authenticated_user(&request, &context.env)?;
    purchases_response(&context.env, &user_id).await
}

async fn purchases_response(env: &Env, user_id: &str) -> Result<Response> {
    let database = env.d1(DATABASE_BINDING)?;
    let rows = worker::query!(
        &database,
        "SELECT order_id, plugin_id, sku, currency, amount, status, created_at
         FROM orders WHERE buyer_user_id = ?1 ORDER BY created_at DESC LIMIT 100",
        user_id
    )?
    .all()
    .await?
    .results::<OrderRow>()?;
    Response::from_json(&json!({"purchases": rows, "nextCursor": null}))
}

async fn active_price(
    database: &worker::D1Database,
    plugin_id: &str,
    sku: &str,
    now: i64,
) -> Result<Option<PriceRow>> {
    worker::query!(
        database,
        "SELECT p.product_id, pr.price_id, p.plugin_id, p.sku,
                p.entitlement_capability AS capability, pr.currency, pr.amount
         FROM products p JOIN prices pr ON pr.product_id = p.product_id
         WHERE p.plugin_id = ?1 AND p.sku = ?2 AND p.active = 1 AND pr.active = 1
           AND pr.starts_at <= ?3 AND (pr.ends_at IS NULL OR pr.ends_at > ?3)
         LIMIT 1",
        plugin_id,
        sku,
        now
    )?
    .first::<PriceRow>(None)
    .await
}

async fn order_by_idempotency(
    database: &worker::D1Database,
    user_id: &str,
    idempotency_key: &str,
) -> Result<Option<OrderRow>> {
    worker::query!(
        database,
        "SELECT order_id, plugin_id, sku, currency, amount, status, created_at
         FROM orders WHERE buyer_user_id = ?1 AND idempotency_key = ?2",
        user_id,
        idempotency_key
    )?
    .first::<OrderRow>(None)
    .await
}

async fn entitlement_for_order(
    database: &worker::D1Database,
    order_id: &str,
) -> Result<Option<Entitlement>> {
    #[derive(Deserialize)]
    struct EntitlementRow {
        entitlement_id: String,
        user_id: String,
        plugin_id: String,
        capability: String,
        expires_at: Option<i64>,
    }
    Ok(worker::query!(
        database,
        "SELECT entitlement_id, user_id, plugin_id, capability, expires_at
         FROM entitlements WHERE order_id = ?1 AND status = 'active' LIMIT 1",
        order_id
    )?
    .first::<EntitlementRow>(None)
    .await?
    .map(|row| Entitlement {
        entitlement_id: row.entitlement_id,
        user_id: row.user_id,
        plugin_id: row.plugin_id,
        capability: row.capability,
        status: EntitlementStatus::Active,
        expires_at: row.expires_at,
    }))
}

async fn current_usage_status(
    database: &worker::D1Database,
    env: &Env,
    user_id: &str,
    now: i64,
) -> Result<AccountUsageStatus> {
    let window_start = usage_window_start(now);
    let window_end = window_start + USAGE_WINDOW_SECONDS;
    let row = worker::query!(
        database,
        "SELECT window_start, window_end, token_limit, used_tokens, reserved_tokens
         FROM ai_usage_budgets WHERE user_id = ?1 AND window_start = ?2",
        user_id,
        window_start
    )?
    .first::<UsageBudgetRow>(None)
    .await?;
    let (token_limit, used_tokens, reserved_tokens) = match row {
        Some(row) => {
            debug_assert_eq!(row.window_start, window_start);
            debug_assert_eq!(row.window_end, window_end);
            (row.token_limit, row.used_tokens, row.reserved_tokens)
        }
        None => (default_usage_limit(env)?, 0, 0),
    };
    Ok(AccountUsageStatus {
        window_start,
        window_end,
        token_limit,
        used_tokens,
        reserved_tokens,
        remaining_tokens: token_limit
            .saturating_sub(used_tokens)
            .saturating_sub(reserved_tokens),
    })
}

async fn usage_reservation_by_request(
    database: &worker::D1Database,
    user_id: &str,
    request_id: &str,
) -> Result<Option<UsageReservationRow>> {
    worker::query!(
        database,
        "SELECT reservation_id, request_id, reserved_tokens, expires_at, state
         FROM ai_usage_reservations WHERE user_id = ?1 AND request_id = ?2",
        user_id,
        request_id
    )?
    .first::<UsageReservationRow>(None)
    .await
}

async fn usage_reservation_by_id(
    database: &worker::D1Database,
    user_id: &str,
    reservation_id: &str,
) -> Result<Option<UsageReservationRow>> {
    worker::query!(
        database,
        "SELECT reservation_id, request_id, reserved_tokens, expires_at, state
         FROM ai_usage_reservations WHERE user_id = ?1 AND reservation_id = ?2",
        user_id,
        reservation_id
    )?
    .first::<UsageReservationRow>(None)
    .await
}

async fn usage_event_by_response(
    database: &worker::D1Database,
    provider_response_id: &str,
) -> Result<Option<UsageEventRow>> {
    worker::query!(
        database,
        "SELECT reservation_id FROM ai_usage_events WHERE provider_response_id = ?1",
        provider_response_id
    )?
    .first::<UsageEventRow>(None)
    .await
}

async fn expire_usage_reservations(
    database: &worker::D1Database,
    user_id: &str,
    now: i64,
) -> Result<()> {
    database
        .batch(vec![
            worker::query!(
                database,
                "UPDATE ai_usage_budgets
                 SET reserved_tokens = reserved_tokens - COALESCE((
                         SELECT SUM(r.reserved_tokens) FROM ai_usage_reservations r
                         WHERE r.user_id = ?1 AND r.window_start = ai_usage_budgets.window_start
                           AND r.state = 'reserved' AND r.expires_at <= ?2
                     ), 0),
                     updated_at = ?2
                 WHERE user_id = ?1
                   AND EXISTS (
                       SELECT 1 FROM ai_usage_reservations r
                       WHERE r.user_id = ?1 AND r.window_start = ai_usage_budgets.window_start
                         AND r.state = 'reserved' AND r.expires_at <= ?2
                   )",
                user_id,
                now
            )?,
            worker::query!(
                database,
                "UPDATE ai_usage_reservations SET state = 'expired', updated_at = ?1
                 WHERE user_id = ?2 AND state = 'reserved' AND expires_at <= ?1
                   AND EXISTS (
                       SELECT 1 FROM ai_usage_budgets b
                       WHERE b.user_id = ?2 AND b.window_start = ai_usage_reservations.window_start
                   )",
                now,
                user_id
            )?,
        ])
        .await?;
    Ok(())
}

fn d1_changes(result: Option<&worker::D1Result>) -> usize {
    result
        .and_then(|result| result.meta().ok().flatten())
        .and_then(|meta| meta.changes)
        .unwrap_or_default()
}

fn usage_window_start(now: i64) -> i64 {
    now - now.rem_euclid(USAGE_WINDOW_SECONDS)
}

fn default_usage_limit(env: &Env) -> Result<i64> {
    let value = env.var("DEFAULT_AI_TOKEN_LIMIT")?.to_string();
    value
        .parse::<i64>()
        .ok()
        .filter(|limit| *limit >= 0)
        .ok_or_else(|| worker::Error::RustError("DEFAULT_AI_TOKEN_LIMIT is invalid".into()))
}

fn require_model_gateway(request: &Request, env: &Env) -> Result<()> {
    let supplied = request
        .headers()
        .get("X-Mahayana-Model-Gateway")?
        .ok_or_else(|| worker::Error::RustError("missing model gateway credential".into()))?;
    let expected = env.secret("MODEL_GATEWAY_TOKEN")?.to_string();
    if !constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
        return Err(worker::Error::RustError(
            "invalid model gateway credential".into(),
        ));
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn usage_limit_response(status: &AccountUsageStatus) -> Result<Response> {
    Ok(Response::from_json(&json!({
        "error": {
            "type": "usage_limit_reached",
            "message": "Mahayana model token limit reached",
            "resets_at": status.window_end,
        },
        "usage": status,
    }))?
    .with_status(429))
}

async fn lookup_login_user(
    database: &worker::D1Database,
    identifier: &str,
) -> Result<Option<AccountUserRow>> {
    let select = "SELECT u.id, u.user_no, u.username, u.username_changed_at, u.email,
                         u.nickname, u.avatar, u.phone_number, u.firebase_uid,
                         u.alipay_user_id, u.alipay_nickname, u.alipay_avatar,
                         u.wechat_headimgurl, u.password_hash, u.salt, u.iterations, u.algo,
                         c.password_phc AS upgraded_password_phc,
                         u.main_practice_title, u.main_practice_file_path,
                         u.main_practice_selected_at, u.created_at, u.email_verified,
                         u.membership_type, u.membership_expires_at, u.free_trial_end_date
                  FROM users u
                  LEFT JOIN account_password_credentials c ON c.user_id = CAST(u.id AS TEXT)";
    let (where_clause, normalized) = if identifier.contains('@') {
        ("LOWER(u.email) = ?1", identifier.to_ascii_lowercase())
    } else if looks_like_phone(identifier) {
        ("u.phone_number = ?1", identifier.to_string())
    } else {
        ("u.username = ?1", identifier.to_string())
    };
    let query = format!("{select} WHERE {where_clause} LIMIT 1");
    worker::query!(database, &query, normalized)?
        .first::<AccountUserRow>(None)
        .await
}

async fn lookup_account_user_by_id(
    database: &worker::D1Database,
    user_id: &str,
) -> Result<Option<AccountUserRow>> {
    worker::query!(
        database,
        "SELECT u.id, u.user_no, u.username, u.username_changed_at, u.email,
                u.nickname, u.avatar, u.phone_number, u.firebase_uid,
                u.alipay_user_id, u.alipay_nickname, u.alipay_avatar,
                u.wechat_headimgurl, u.password_hash, u.salt, u.iterations, u.algo,
                c.password_phc AS upgraded_password_phc,
                u.main_practice_title, u.main_practice_file_path,
                u.main_practice_selected_at, u.created_at, u.email_verified,
                u.membership_type, u.membership_expires_at, u.free_trial_end_date
         FROM users u
         LEFT JOIN account_password_credentials c ON c.user_id = CAST(u.id AS TEXT)
         WHERE CAST(u.id AS TEXT) = ?1 OR u.username = ?1
         LIMIT 1",
        user_id
    )?
    .first::<AccountUserRow>(None)
    .await
}

fn looks_like_phone(value: &str) -> bool {
    let value = value.strip_prefix('+').unwrap_or(value);
    (6..=20).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn normalize_device_id(device_id: Option<&str>) -> Result<String> {
    let device_id = device_id.map(str::trim).filter(|value| !value.is_empty());
    let Some(device_id) = device_id else {
        return Ok(format!("device:{}", Uuid::new_v4()));
    };
    if device_id.len() > 128
        || !device_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(worker::Error::RustError("invalid device id".into()));
    }
    Ok(device_id.to_string())
}

fn issue_account_access_token(
    env: &Env,
    user_id: &str,
    device_id: &str,
    session_id: &str,
    now: i64,
) -> Result<(String, i64, String)> {
    let expires_at = now + ACCESS_TOKEN_SECONDS;
    let jti = Uuid::new_v4().to_string();
    let claims = AccountAccessTokenClaims {
        iss: ACCESS_TOKEN_ISSUER.to_string(),
        sub: user_id.to_string(),
        aud: ACCESS_TOKEN_AUDIENCE.to_string(),
        scope: vec![
            "account.read".to_string(),
            "marketplace.read".to_string(),
            "wallet.read".to_string(),
            "commerce.purchase".to_string(),
            "model.invoke".to_string(),
        ],
        device_id: device_id.to_string(),
        sid: session_id.to_string(),
        jti: jti.clone(),
        iat: usize::try_from(now).unwrap_or_default(),
        exp: usize::try_from(expires_at).unwrap_or(usize::MAX),
        token_use: "access".to_string(),
    };
    let private_key = env.secret("ACCESS_TOKEN_PRIVATE_KEY_PEM")?.to_string();
    let key = EncodingKey::from_rsa_pem(private_key.as_bytes()).map_err(jwt_error)?;
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("JWT".to_string());
    header.kid = Some(env.var("ACCESS_TOKEN_KEY_ID")?.to_string());
    let token = encode(&header, &claims, &key).map_err(jwt_error)?;
    Ok((token, expires_at, jti))
}

fn serialize_account_user(user: &AccountUserRow) -> serde_json::Value {
    let avatar = user
        .avatar
        .as_ref()
        .or(user.alipay_avatar.as_ref())
        .or(user.wechat_headimgurl.as_ref());
    let main_practice = user.main_practice_title.as_ref().map(|title| {
        json!({
            "title": title,
            "filePath": user.main_practice_file_path,
            "selectedAt": user.main_practice_selected_at,
        })
    });
    json!({
        "id": user.id,
        "userId": user.id,
        "userNo": user.user_no.unwrap_or(user.id),
        "username": user.username,
        "usernameChangedAt": user.username_changed_at,
        "email": user.email.as_deref().unwrap_or_default(),
        "nickname": user.nickname.as_deref().unwrap_or(&user.username),
        "avatar": avatar,
        "phoneNumber": user.phone_number,
        "firebaseUid": user.firebase_uid,
        "alipayProviderSubject": user.alipay_user_id,
        "alipayUserId": user.alipay_user_id,
        "alipayNickname": user.alipay_nickname,
        "alipayAvatar": user.alipay_avatar,
        "hasPassword": user.password_hash.is_some() && user.salt.is_some(),
        "mainPractice": main_practice,
        "createdAt": user.created_at,
        "emailVerified": user.email_verified == Some(1),
        "membership": {
            "type": user.membership_type.as_deref().unwrap_or("expired"),
            "expiresAt": user.membership_expires_at.as_ref().or(user.free_trial_end_date.as_ref()),
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn account_session_response(
    user: &AccountUserRow,
    access_token: &str,
    refresh_token: &str,
    access_expires_at: i64,
    refresh_expires_at: i64,
    session_id: &str,
    device_id: &str,
) -> Result<Response> {
    Ok(Response::from_json(&json!({
        "accessToken": access_token,
        "refreshToken": refresh_token,
        "tokenType": "Bearer",
        "expiresIn": ACCESS_TOKEN_SECONDS,
        "accessTokenExpiresAt": access_expires_at,
        "refreshTokenExpiresAt": refresh_expires_at,
        "sessionId": session_id,
        "deviceId": device_id,
        "username": user.username,
        "userId": user.id,
        "userNo": user.user_no.unwrap_or(user.id),
        "user": serialize_account_user(user),
    }))?
    .with_headers(auth_headers()))
}

async fn record_auth_event(
    database: &worker::D1Database,
    user_id: Option<&str>,
    session_id: Option<&str>,
    event_type: &str,
    now: i64,
) -> Result<()> {
    worker::query!(
        database,
        "INSERT INTO account_auth_events
         (event_id, user_id, session_id, event_type, occurred_at, details_json)
         VALUES (?1, ?2, ?3, ?4, ?5, '{}')",
        Uuid::new_v4().to_string(),
        user_id,
        session_id,
        event_type,
        now
    )?
    .run()
    .await?;
    Ok(())
}

async fn account_login_is_rate_limited(
    database: &worker::D1Database,
    user_id: &str,
    now: i64,
) -> Result<bool> {
    let window_start = now - LOGIN_FAILURE_WINDOW_SECONDS;
    let count = worker::query!(
        database,
        "SELECT COUNT(*) AS failure_count
         FROM account_auth_events
         WHERE user_id = ?1 AND event_type = 'login_failed' AND occurred_at >= ?2",
        user_id,
        window_start
    )?
    .first::<LoginFailureCountRow>(None)
    .await?
    .map(|row| row.failure_count)
    .unwrap_or_default();
    Ok(count >= MAX_ACCOUNT_LOGIN_FAILURES)
}

async fn revoke_account_session(
    database: &worker::D1Database,
    session_id: &str,
    reason: &str,
    now: i64,
) -> Result<()> {
    let event_id = Uuid::new_v4().to_string();
    database
        .batch(vec![
            worker::query!(
                database,
                "UPDATE account_sessions
                 SET revoked_at = COALESCE(revoked_at, ?1), revoked_reason = COALESCE(revoked_reason, ?2)
                 WHERE session_id = ?3",
                now,
                reason,
                session_id
            )?,
            worker::query!(
                database,
                "UPDATE account_refresh_tokens SET state = 'revoked'
                 WHERE session_id = ?1 AND state = 'active'",
                session_id
            )?,
            worker::query!(
                database,
                "INSERT INTO account_auth_events
                 (event_id, user_id, session_id, event_type, occurred_at, details_json)
                 SELECT ?1, user_id, session_id, ?2, ?3, '{}'
                 FROM account_sessions WHERE session_id = ?4",
                &event_id,
                reason,
                now,
                session_id
            )?,
        ])
        .await?;
    Ok(())
}

fn authenticated_user(request: &Request, env: &Env) -> Result<String> {
    Ok(authenticated_account(request, env)?.user_id)
}

fn authenticated_account(request: &Request, env: &Env) -> Result<AuthenticatedAccount> {
    let authorization = request
        .headers()
        .get("Authorization")?
        .ok_or_else(|| worker::Error::RustError("missing Authorization header".into()))?;
    let token = authorization
        .strip_prefix("Bearer ")
        .ok_or_else(|| worker::Error::RustError("invalid Authorization scheme".into()))?;
    let public_key = env.secret("ACCESS_TOKEN_PUBLIC_KEY_PEM")?.to_string();
    let key = DecodingKey::from_rsa_pem(public_key.as_bytes()).map_err(jwt_error)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ACCESS_TOKEN_ISSUER]);
    validation.set_audience(&[ACCESS_TOKEN_AUDIENCE]);
    let claims = decode::<AccountAccessTokenClaims>(token, &key, &validation)
        .map_err(jwt_error)?
        .claims;
    if claims.token_use != "access"
        || claims.sub.trim().is_empty()
        || claims.sid.trim().is_empty()
        || claims.device_id.trim().is_empty()
    {
        return Err(worker::Error::RustError(
            "invalid access token claims".into(),
        ));
    }
    Ok(AuthenticatedAccount {
        user_id: claims.sub,
        session_id: Some(claims.sid),
    })
}

fn route_identifier<'a>(context: &'a RouteContext<()>, name: &str) -> Result<&'a str> {
    let value = context
        .param(name)
        .map(String::as_str)
        .filter(|value| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .ok_or_else(|| worker::Error::RustError(format!("invalid route parameter {name}")))?;
    Ok(value)
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_scope(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
        })
}

fn is_opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'"' | b'\'' | b'\\'))
}

fn now_seconds() -> i64 {
    (Date::now().as_millis() / 1_000) as i64
}

fn jwt_error(error: jsonwebtoken::errors::Error) -> worker::Error {
    worker::Error::RustError(format!("access token verification failed: {error}"))
}

fn json_headers() -> worker::Headers {
    let headers = worker::Headers::new();
    let _ = headers.set("Content-Type", "application/json; charset=utf-8");
    let _ = headers.set("Access-Control-Allow-Origin", "*");
    let _ = headers.set("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
    let _ = headers.set(
        "Access-Control-Allow-Headers",
        "Content-Type, Authorization, X-Mahayana-Model-Gateway",
    );
    headers
}

fn auth_headers() -> worker::Headers {
    let headers = json_headers();
    let _ = headers.set("Cache-Control", "no-store");
    let _ = headers.set("Pragma", "no-cache");
    headers
}

fn error_response(status: u16, code: &str, message: &str) -> Result<Response> {
    Ok(
        Response::from_json(&json!({"error": code, "message": message}))?
            .with_headers(auth_headers())
            .with_status(status),
    )
}
