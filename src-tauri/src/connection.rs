use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum DesktopConnection {
    Personal,
    Enterprise {
        #[serde(rename = "portalOrigin")]
        portal_origin: String,
        #[serde(rename = "organizationId", skip_serializing_if = "Option::is_none")]
        organization_id: Option<String>,
        #[serde(rename = "principalId", skip_serializing_if = "Option::is_none")]
        principal_id: Option<String>,
    },
}

/// Backend configuration is independent of the local official Host address.
pub fn enterprise_connection(value: &str) -> Result<DesktopConnection, String> {
    let url = url::Url::parse(value).map_err(|_| "企业地址必须使用 HTTPS；仅本机测试允许 HTTP".to_string())?;
    let loopback = matches!(url.host_str(), Some("localhost") | Some("127.0.0.1") | Some("[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("企业地址必须使用 HTTPS；仅本机测试允许 HTTP".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        return Err("请输入企业后台地址，不包含账号、令牌或页面路径".to_string());
    }
    Ok(DesktopConnection::Enterprise {
        portal_origin: url.origin().ascii_serialization(),
        organization_id: None,
        principal_id: None,
    })
}

/// Ephemeral enterprise browser state is separate from the existing personal session.
pub fn connection_partition(connection: &DesktopConnection) -> Option<String> {
    match connection {
        DesktopConnection::Personal => None,
        DesktopConnection::Enterprise { portal_origin, organization_id, principal_id } => {
            let origin = match enterprise_connection(portal_origin) {
                Ok(DesktopConnection::Enterprise { portal_origin, .. }) => portal_origin,
                _ => portal_origin.clone(),
            };
            Some(format!(
                "quantdsh-enterprise-{}",
                enterprise_namespace(&origin, organization_id.as_deref().unwrap_or(""), principal_id.as_deref().unwrap_or(""))
            ))
        }
    }
}

pub fn enterprise_namespace(origin: &str, organization_id: &str, principal_id: &str) -> String {
    // JSON.stringify([portalOrigin, organizationId, principalId]) has no spaces.
    let payload = serde_json::to_string(&[origin, organization_id, principal_id])
        .unwrap_or_else(|_| format!("[{}, {}, {}]", serde_json::to_string(origin).unwrap_or_default(), serde_json::to_string(organization_id).unwrap_or_default(), serde_json::to_string(principal_id).unwrap_or_default()));
    let digest = Sha256::digest(payload.as_bytes());
    hex::encode(digest)
}
