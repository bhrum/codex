//! Protocol-facing state machines and schema coverage checks.
//!
//! Network transports and generated TL constructors will be added behind this
//! boundary. The current API makes authorization transitions and upstream drift
//! independently testable before a specific transport is selected.

pub mod api;
pub mod auth;
pub mod crypto;
pub mod dc;
pub mod generated;
pub mod handshake;
pub mod request;
pub mod schema;
pub mod session;
pub mod srp;
pub mod tl;
pub mod transport;
pub mod updates;

pub use api::{
    build_account_get_password, build_auth_check_password, build_auth_send_code,
    build_auth_sign_in, build_auth_sign_up, build_init_connection_get_config, build_msgs_ack,
    parse_account_password_prefix, parse_auth_sent_code, parse_config_dc_directory_prefix,
    try_parse_rpc_error, AccountPasswordState, ApiRequestError, ConfigDcDirectory, InitConnection,
    NextCodeType, RpcErrorResponse, SentCode, SentCodeDelivery, SentCodeResult,
    ACCOUNT_GET_PASSWORD_CONSTRUCTOR, ACCOUNT_PASSWORD_CONSTRUCTOR,
    AUTH_CHECK_PASSWORD_CONSTRUCTOR, AUTH_SEND_CODE_CONSTRUCTOR, AUTH_SENT_CODE_CONSTRUCTOR,
    AUTH_SENT_CODE_PAYMENT_REQUIRED_CONSTRUCTOR, AUTH_SENT_CODE_SUCCESS_CONSTRUCTOR,
    AUTH_SIGN_IN_CONSTRUCTOR, AUTH_SIGN_UP_CONSTRUCTOR, CODE_SETTINGS_CONSTRUCTOR,
    CONFIG_CONSTRUCTOR, DC_OPTION_CONSTRUCTOR, HELP_GET_CONFIG_CONSTRUCTOR,
    INIT_CONNECTION_CONSTRUCTOR, INVOKE_WITH_LAYER_CONSTRUCTOR, MSGS_ACK_CONSTRUCTOR,
    MTPROTO_LAYER, PASSWORD_KDF_ALGO_CONSTRUCTOR, RPC_ERROR_CONSTRUCTOR,
};
pub use auth::{
    AuthCommand, AuthError, AuthEvent, AuthorizationMachine, AuthorizationState, CodeDeliveryType,
};
pub use crypto::{
    aes_ige_decrypt, aes_ige_encrypt, decrypt_message, derive_aes_key_iv, encrypt_message,
    quick_ack_token, AuthKey, CryptoDirection, CryptoError, EncryptedEnvelope, PlainMessage,
};
pub use dc::{
    classify_rpc_error, parse_migration, DcDirectory, DcEndpoint, DcError, DcPurpose, DcRoute,
    MigrationDirective, MigrationKind, RpcErrorAction,
};
pub use generated::{
    wire_constructor_by_name, wire_constructors_by_id, WireConstructor, WireSchema,
    WIRE_CONSTRUCTORS,
};
pub use handshake::{
    build_p_q_inner_data_dc, build_req_dh_params, decrypt_server_dh_inner_data,
    derive_tmp_aes_key_iv, factor_res_pq, parse_dh_gen_result, parse_res_pq,
    parse_server_dh_params, prepare_client_dh, rsa_pad, rsa_pad_with_random,
    select_server_key_fingerprint, telegram_server_rsa_key, validate_server_dh_parameters,
    AuthKeyHandshake, AuthKeyHandshakeState, DhGenAction, DhGenResult, EstablishedAuthKey,
    FactoredPq, HandshakeError, Nonce, PlaintextEnvelope, PreparedClientDh, ResPq, RsaPublicKey,
    ServerDhInnerData, ServerDhParams, CLIENT_DH_INNER_DATA_CONSTRUCTOR,
    DEFAULT_MAX_PLAINTEXT_BODY, DH_GEN_FAIL_CONSTRUCTOR, DH_GEN_OK_CONSTRUCTOR,
    DH_GEN_RETRY_CONSTRUCTOR, KNOWN_DH_PRIME_HEX, P_Q_INNER_DATA_DC_CONSTRUCTOR,
    REQ_DH_PARAMS_CONSTRUCTOR, REQ_PQ_MULTI_CONSTRUCTOR, RES_PQ_CONSTRUCTOR,
    SERVER_DH_INNER_DATA_CONSTRUCTOR, SERVER_DH_PARAMS_FAIL_CONSTRUCTOR,
    SERVER_DH_PARAMS_OK_CONSTRUCTOR, SET_CLIENT_DH_PARAMS_CONSTRUCTOR,
    TELEGRAM_MAIN_RSA_MODULUS_HEX, TELEGRAM_TEST_RSA_MODULUS_HEX,
};
pub use request::{ProtocolRequest, RequestId, RequestSequencer};
pub use schema::{
    audit_mtproto_api_schema, audit_schema, audit_td_api_schema, audit_telegram_api_schema,
    parse_schema_catalog, parse_td_api_schema, DeclarationKind, SchemaAudit, SchemaBaseline,
    SchemaCatalog, SchemaDeclaration, SchemaError, SchemaParseError, SchemaStats,
    MTPROTO_API_BASELINE, MTPROTO_API_EXPECTED_FUNCTIONS, MTPROTO_API_EXPECTED_SHA256,
    MTPROTO_API_EXPECTED_TYPES, TD_API_BASELINE, TD_API_EXPECTED_FUNCTIONS, TD_API_EXPECTED_SHA256,
    TD_API_EXPECTED_TYPES, TD_API_PINNED_COMMIT, TELEGRAM_API_BASELINE,
    TELEGRAM_API_EXPECTED_FUNCTIONS, TELEGRAM_API_EXPECTED_SHA256, TELEGRAM_API_EXPECTED_TYPES,
};
pub use session::{MessageIdError, MessageIdGuard, SequenceError, SessionSequence};
pub use srp::{
    compute_password_srp_proof, compute_password_srp_proof_with_random, PasswordSrpParameters,
    PasswordSrpProof, SrpError, INPUT_CHECK_PASSWORD_SRP_CONSTRUCTOR,
};
pub use tl::{TlError, TlReader, TlWriter, BOOL_FALSE_CONSTRUCTOR, BOOL_TRUE_CONSTRUCTOR};
pub use transport::{DecodedFrame, TransportError, TransportFrameCodec, TransportMode};
pub use updates::{
    build_updates_get_difference, build_updates_get_state, parse_terminal_difference,
    parse_update_state, DifferenceRequest, TerminalDifference, UpdateError, UpdateState,
    UPDATES_DIFFERENCE_EMPTY_CONSTRUCTOR, UPDATES_DIFFERENCE_TOO_LONG_CONSTRUCTOR,
    UPDATES_GET_DIFFERENCE_CONSTRUCTOR, UPDATES_GET_STATE_CONSTRUCTOR, UPDATES_STATE_CONSTRUCTOR,
};
