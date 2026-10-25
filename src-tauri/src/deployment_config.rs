use serde_json::Value;

/// Administrator-owned packaging input. Never accepts credentials or arbitrary settings.
pub fn parse_deployment_config(value: &Value) -> Result<Value, String> {
    if !value.is_object() {
        return Err("打包配置必须是 JSON 对象".to_string());
    }
    let root = value.as_object().unwrap();
    if root.keys().any(|key| key != "enterprise") {
        return Err("打包配置仅允许 enterprise 后台地址，不接受密钥或其他设置".to_string());
    }
    let Some(enterprise) = root.get("enterprise") else {
        return Ok(serde_json::json!({}));
    };
    if !enterprise.is_object() {
        return Err("enterprise 配置必须包含 backendUrl".to_string());
    }
    let enterprise = enterprise.as_object().unwrap();
    if enterprise.keys().any(|key| key != "backendUrl") {
        return Err("enterprise 配置仅允许 backendUrl，不接受账号或密钥".to_string());
    }
    let Some(backend_url) = enterprise.get("backendUrl").and_then(Value::as_str) else {
        return Err("enterprise 配置仅允许 backendUrl，不接受账号或密钥".to_string());
    };
    let connection = crate::connection::enterprise_connection(backend_url)?;
    let origin = match connection {
        crate::connection::DesktopConnection::Enterprise { portal_origin, .. } => portal_origin,
        _ => unreachable!(),
    };
    Ok(serde_json::json!({ "enterprise": { "backendUrl": origin } }))
}
