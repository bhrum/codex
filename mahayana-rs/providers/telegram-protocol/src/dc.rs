use std::{collections::BTreeSet, net::IpAddr};

use thiserror::Error;

const TELEGRAM_PORTS: [u16; 3] = [443, 80, 5222];
type BootstrapEndpoint = (i32, &'static str);
type BootstrapEndpoints = &'static [BootstrapEndpoint];
const TELEGRAM_PRODUCTION_IPV4: [(i32, &str); 6] = [
    (1, "149.154.175.50"),
    (2, "149.154.167.51"),
    (2, "95.161.76.100"),
    (3, "149.154.175.100"),
    (4, "149.154.167.91"),
    (5, "149.154.171.5"),
];
const TELEGRAM_PRODUCTION_IPV6: [(i32, &str); 5] = [
    (1, "2001:b28:f23d:f001::a"),
    (2, "2001:67c:4e8:f002::a"),
    (3, "2001:b28:f23d:f003::a"),
    (4, "2001:67c:4e8:f004::a"),
    (5, "2001:b28:f23f:f005::a"),
];
const TELEGRAM_TEST_IPV4: [(i32, &str); 3] = [
    (1, "149.154.175.10"),
    (2, "149.154.167.40"),
    (3, "149.154.175.117"),
];
const TELEGRAM_TEST_IPV6: [(i32, &str); 3] = [
    (1, "2001:b28:f23d:f001::e"),
    (2, "2001:67c:4e8:f002::e"),
    (3, "2001:b28:f23d:f003::e"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DcPurpose {
    Main,
    Media,
    Cdn,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DcEndpoint {
    pub dc_id: i32,
    pub ip_address: IpAddr,
    pub port: u16,
    pub media_only: bool,
    pub tcpo_only: bool,
    pub cdn: bool,
    pub is_static: bool,
    pub this_port_only: bool,
    pub secret: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DcDirectory {
    this_dc: i32,
    test_mode: bool,
    endpoints: Vec<DcEndpoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationKind {
    Phone,
    Network,
    User,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationDirective {
    pub kind: MigrationKind,
    pub dc_id: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcErrorAction {
    Migrate(MigrationDirective),
    Wait { seconds: u32, premium: bool },
    Reauthorize,
    RetryTransient,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DcRoute {
    SwitchMain { dc_id: i32, reason: MigrationKind },
    RetryOn { dc_id: i32, purpose: DcPurpose },
    Wait { seconds: u32, premium: bool },
    Reauthorize,
    RetryTransient,
    Fail,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DcError {
    #[error("data center id must be positive")]
    InvalidDcId,
    #[error("data center endpoint port must be non-zero")]
    InvalidPort,
    #[error("data center directory must contain at least one endpoint")]
    EmptyDirectory,
    #[error("data center directory contains a duplicate endpoint")]
    DuplicateEndpoint,
    #[error("data center {dc_id} has no endpoint for {purpose:?}")]
    MissingEndpoint { dc_id: i32, purpose: DcPurpose },
}

impl DcEndpoint {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dc_id: i32,
        ip_address: IpAddr,
        port: u16,
        media_only: bool,
        tcpo_only: bool,
        cdn: bool,
        is_static: bool,
        this_port_only: bool,
        secret: Option<Vec<u8>>,
    ) -> Result<Self, DcError> {
        if dc_id <= 0 {
            return Err(DcError::InvalidDcId);
        }
        if port == 0 {
            return Err(DcError::InvalidPort);
        }
        Ok(Self {
            dc_id,
            ip_address,
            port,
            media_only,
            tcpo_only,
            cdn,
            is_static,
            this_port_only,
            secret,
        })
    }

    fn supports(&self, purpose: DcPurpose) -> bool {
        match purpose {
            DcPurpose::Main => !self.media_only && !self.cdn,
            DcPurpose::Media => !self.cdn,
            DcPurpose::Cdn => self.cdn,
        }
    }
}

impl DcDirectory {
    /// Returns TDLib's pinned bootstrap endpoints. Telegram replaces this
    /// directory with `help.getConfig` after an authorized session starts.
    pub fn telegram_defaults(test_mode: bool, this_dc: i32) -> Result<Self, DcError> {
        let (ipv4, ipv6): (BootstrapEndpoints, BootstrapEndpoints) = if test_mode {
            (&TELEGRAM_TEST_IPV4, &TELEGRAM_TEST_IPV6)
        } else {
            (&TELEGRAM_PRODUCTION_IPV4, &TELEGRAM_PRODUCTION_IPV6)
        };
        let mut endpoints = Vec::with_capacity((ipv4.len() + ipv6.len()) * TELEGRAM_PORTS.len());
        for (dc_id, address) in ipv4.iter().chain(ipv6.iter()) {
            let ip_address = address
                .parse()
                .expect("pinned Telegram bootstrap address must be valid");
            for port in TELEGRAM_PORTS {
                endpoints.push(DcEndpoint::new(
                    *dc_id, ip_address, port, false, false, false, true, false, None,
                )?);
            }
        }
        Self::new(this_dc, test_mode, endpoints)
    }

    pub fn new(
        this_dc: i32,
        test_mode: bool,
        mut endpoints: Vec<DcEndpoint>,
    ) -> Result<Self, DcError> {
        if this_dc <= 0 {
            return Err(DcError::InvalidDcId);
        }
        if endpoints.is_empty() {
            return Err(DcError::EmptyDirectory);
        }

        let mut unique = BTreeSet::new();
        for endpoint in &endpoints {
            if !unique.insert(endpoint.clone()) {
                return Err(DcError::DuplicateEndpoint);
            }
        }

        endpoints.sort_by_key(|endpoint| {
            (
                endpoint.dc_id,
                endpoint.cdn,
                endpoint.media_only,
                endpoint.ip_address.is_ipv6(),
                endpoint.port,
            )
        });

        let directory = Self {
            this_dc,
            test_mode,
            endpoints,
        };
        directory.candidates(this_dc, DcPurpose::Main, false)?;
        Ok(directory)
    }

    pub fn this_dc(&self) -> i32 {
        self.this_dc
    }

    pub fn test_mode(&self) -> bool {
        self.test_mode
    }

    pub fn endpoints(&self) -> &[DcEndpoint] {
        &self.endpoints
    }

    pub fn update(
        &mut self,
        this_dc: i32,
        test_mode: bool,
        endpoints: Vec<DcEndpoint>,
    ) -> Result<(), DcError> {
        let replacement = Self::new(this_dc, test_mode, endpoints)?;
        *self = replacement;
        Ok(())
    }

    pub fn candidates(
        &self,
        dc_id: i32,
        purpose: DcPurpose,
        prefer_ipv6: bool,
    ) -> Result<Vec<&DcEndpoint>, DcError> {
        if dc_id <= 0 {
            return Err(DcError::InvalidDcId);
        }
        let mut candidates: Vec<_> = self
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.dc_id == dc_id && endpoint.supports(purpose))
            .collect();
        candidates.sort_by_key(|endpoint| {
            (
                endpoint.ip_address.is_ipv6() != prefer_ipv6,
                purpose == DcPurpose::Media && !endpoint.media_only,
                endpoint.tcpo_only,
                !endpoint.is_static,
                endpoint.port,
            )
        });
        if candidates.is_empty() {
            return Err(DcError::MissingEndpoint { dc_id, purpose });
        }
        Ok(candidates)
    }

    pub fn route_rpc_error(&self, code: i32, message: &str) -> Result<DcRoute, DcError> {
        match classify_rpc_error(code, message) {
            RpcErrorAction::Migrate(directive) => {
                let purpose = if directive.kind == MigrationKind::File {
                    DcPurpose::Media
                } else {
                    DcPurpose::Main
                };
                self.candidates(directive.dc_id, purpose, false)?;
                if directive.kind == MigrationKind::File {
                    Ok(DcRoute::RetryOn {
                        dc_id: directive.dc_id,
                        purpose,
                    })
                } else {
                    Ok(DcRoute::SwitchMain {
                        dc_id: directive.dc_id,
                        reason: directive.kind,
                    })
                }
            }
            RpcErrorAction::Wait { seconds, premium } => Ok(DcRoute::Wait { seconds, premium }),
            RpcErrorAction::Reauthorize => Ok(DcRoute::Reauthorize),
            RpcErrorAction::RetryTransient => Ok(DcRoute::RetryTransient),
            RpcErrorAction::Fail => Ok(DcRoute::Fail),
        }
    }
}

pub fn classify_rpc_error(code: i32, message: &str) -> RpcErrorAction {
    let message = message.trim();
    if code == 303 {
        if let Some(directive) = parse_migration(message) {
            return RpcErrorAction::Migrate(directive);
        }
        return RpcErrorAction::Fail;
    }

    if code == 420 {
        if let Some(seconds) = parse_positive_suffix(message, "FLOOD_WAIT_") {
            return RpcErrorAction::Wait {
                seconds,
                premium: false,
            };
        }
        if let Some(seconds) = parse_positive_suffix(message, "FLOOD_PREMIUM_WAIT_") {
            return RpcErrorAction::Wait {
                seconds,
                premium: true,
            };
        }
        return RpcErrorAction::Fail;
    }

    if code == 401
        && matches!(
            message,
            "AUTH_KEY_UNREGISTERED"
                | "AUTH_KEY_INVALID"
                | "SESSION_REVOKED"
                | "SESSION_EXPIRED"
                | "AUTH_KEY_DUPLICATED"
        )
    {
        return RpcErrorAction::Reauthorize;
    }

    if code >= 500 {
        RpcErrorAction::RetryTransient
    } else {
        RpcErrorAction::Fail
    }
}

pub fn parse_migration(message: &str) -> Option<MigrationDirective> {
    [
        ("PHONE_MIGRATE_", MigrationKind::Phone),
        ("NETWORK_MIGRATE_", MigrationKind::Network),
        ("USER_MIGRATE_", MigrationKind::User),
        ("FILE_MIGRATE_", MigrationKind::File),
    ]
    .into_iter()
    .find_map(|(prefix, kind)| {
        parse_positive_suffix(message, prefix).and_then(|dc_id| {
            Some(MigrationDirective {
                kind,
                dc_id: dc_id.try_into().ok()?,
            })
        })
    })
}

fn parse_positive_suffix(message: &str, prefix: &str) -> Option<u32> {
    let suffix = message.strip_prefix(prefix)?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = suffix.parse().ok()?;
    (value > 0).then_some(value)
}
