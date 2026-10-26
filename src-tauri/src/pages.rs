//! Carrier connection surfaces, byte-compatible with the upstream desktop's connection-entry.ts.
//! The pages are served through the registered `quantdsh://` protocol and carry the
//! same Content-Security-Policy as the Electron data: documents.

fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for char in value.chars() {
        match char {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Everything before `<main>`, including the `<style>` block.
/// `enterprise_login_html` rewrites the CSP form-action of this head.
/// The head is a fixed document: returned as a plain literal, because the CSS
/// braces must stay literal (a `format!` would require escaping every one).
fn carrier_head() -> String {
    r#"<!doctype html><html lang="zh-CN"><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; form-action quantdsh:; base-uri 'none'">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>QuantDSH</title>
<style>:root{color-scheme:light dark;--bg:#f5f7fa;--panel:#fff;--text:#171a20;--muted:#626b78;--border:#dce1e8}*{box-sizing:border-box}[hidden]{display:none!important}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.6 -apple-system,BlinkMacSystemFont,"PingFang SC","Microsoft YaHei",sans-serif;display:grid;min-height:100vh;place-items:center}main{width:min(440px,calc(100vw - 48px));padding:32px;background:var(--panel);border:1px solid var(--border);border-radius:16px}header{font-size:22px;font-weight:600;margin-bottom:24px}header span{color:#2463eb}h1{font-size:22px;margin:0 0 8px}p{color:var(--muted);margin:0 0 20px}a,button{display:block;width:100%;text-align:center;border-radius:8px;padding:10px 16px;font:inherit;text-decoration:none;border:1px solid var(--border);background:var(--panel);color:var(--text);cursor:pointer}button{background:#2463eb;color:white;border-color:#2463eb;margin-top:12px}form{margin-top:24px;padding-top:24px;border-top:1px solid var(--border)}label{display:block;margin-bottom:8px}input{width:100%;padding:10px 12px;border:1px solid var(--border);border-radius:8px;background:var(--panel);color:var(--text);font:inherit}a:focus-visible,button:focus-visible,input:focus-visible{outline:3px solid #80aaff;outline-offset:2px}@media(prefers-color-scheme:dark){:root{--bg:#121212;--panel:#202020;--text:#e7e7e7;--muted:#a5a5a5;--border:#414141}}</style>
"#
    .to_string()
}

/// quantdsh `connectionEntryHtml`.
pub fn connection_entry_html(portal: &str, managed: bool, enterprise_available: bool) -> String {
    let escaped = escape_html(portal);
    let backend = if managed {
        format!("<label>企业后台</label><p>{escaped}</p>")
    } else {
        format!(
            "<label for=\"portal\">企业后台地址</label><input id=\"portal\" name=\"portal\" type=\"url\" required placeholder=\"https://企业地址\" value=\"{escaped}\" autocomplete=\"url\">"
        )
    };
    let submit = if enterprise_available {
        "<button type=\"submit\">企业登录</button>".to_string()
    } else {
        "<button type=\"button\" disabled>请先安装企业账号插件</button><p>进入个人空间，在插件管理中安装企业插件，再从工作区菜单切换到企业登录。</p>".to_string()
    };
    format!(
        "{}<main><header><span>Q</span> QuantDSH</header><h1>选择使用方式</h1><p>官方 DSH 在本机运行。个人与企业使用独立空间。</p><a href=\"quantdsh://personal\">个人使用</a><form action=\"quantdsh://enterprise\" method=\"get\">{backend}{submit}</form></main></html>",
        carrier_head(),
    )
}

/// quantdsh `enterpriseLoginHtml`.
pub fn enterprise_login_html(backend: &str) -> String {
    let base = connection_entry_html(backend, false, true);
    let start = base.find("<main>").unwrap_or(base.len());
    let escaped = escape_html(backend);
    // String.prototype.replace with a string pattern replaces the first occurrence only.
    let head = base[..start].replacen("form-action quantdsh:", "form-action 'none'", 1);
    format!(
        r#"{head}<main><header><span>Q</span> QuantDSH</header><h1>登录企业 Desktop</h1><p>{escaped}</p><form id="enterprise-login"><label for="account">账号</label><input id="account" autocomplete="username" required maxlength="254"><label for="password" style="margin-top:16px">密码</label><input id="password" type="password" autocomplete="current-password" required maxlength="512"><button type="submit">登录并在本机运行</button></form><p id="status" role="status" style="margin-top:16px"></p><a href="quantdsh://entry" style="margin-top:16px">返回使用方式</a></main></html>"#
    )
}

/// Tauri-only repair surface for an incomplete runtime pack.
///
/// The Electron carrier answered this situation with
/// `throw new Error('Bundled Node.js runtime is missing: …')` plus a generic
/// error box and a dead window. The contract is unchanged — no host process is
/// ever started against a partial tree — but the failure is now actionable.
pub fn runtime_missing_html(
    source_label: &str,
    root: &str,
    problems: &[String],
    download_url: Option<&str>,
    install_root: &str,
) -> String {
    let base = connection_entry_html("", false, true);
    let head = &base[..base.find("<main>").unwrap_or(base.len())];
    let shown = problems.len().min(8);
    let mut items = String::new();
    for problem in problems.iter().take(shown) {
        items.push_str("<li>");
        items.push_str(&escape_html(problem));
        items.push_str("</li>");
    }
    if problems.len() > shown {
        items.push_str(&format!("<li>…等 {} 项</li>", problems.len()));
    }
    let download = match download_url {
        Some(_) => r#"<button type="button" id="runtime-download">在浏览器中打开下载页</button>"#.to_string(),
        None => r#"<p class="hint">本安装包未配置在线下载地址，请联系管理员获取离线包（runtime-&lt;版本&gt;-&lt;平台&gt;.tar.zst）。</p>"#.to_string(),
    };
    format!(
        r#"{head}<style>main{{width:min(560px,calc(100vw - 48px))}}.problems{{margin:0 0 20px;padding-left:20px;color:var(--muted);word-break:break-all}}.problems li{{margin:4px 0}}.hint{{color:var(--muted);font-size:12px;word-break:break-all}}button.secondary{{background:var(--panel);color:var(--text);border-color:var(--border)}}code{{font:12px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;word-break:break-all}}</style><main><header><span>Q</span> QuantDSH</header><h1>本机运行时不可用</h1><p>{source_label} 未通过完整性校验，因此没有启动本机 DSH。安装包可能未完整解压，或被安全软件清除了文件。</p><ul class="problems">{items}</ul><div id="runtime-actions"><button type="button" id="runtime-import">导入运行时离线包</button>{download}<button type="button" id="runtime-exit" class="secondary">退出</button></div><p id="runtime-status" role="status" style="margin-top:16px"></p><p class="hint">当前检查的目录：<code>{root}</code><br>导入的离线包将安装到：<code>{install_root}</code><br>导入成功后会立即重启 QuantDSH。</p></main></html>"#,
        head = head,
        source_label = escape_html(source_label),
        items = items,
        download = download,
        root = escape_html(root),
        install_root = escape_html(install_root),
    )
}

/// quantdsh `connectionLoadingHtml`.
pub fn connection_loading_html(message: &str) -> String {
    let base = connection_entry_html("", false, true);
    let head = &base[..base.find("<main>").unwrap_or(base.len())];
    let escaped = escape_html(message);
    format!(
        r#"{head}<style>main{{animation:appear .16s ease-out}}.spinner{{width:28px;height:28px;border:3px solid var(--border);border-top-color:#2463eb;border-radius:50%;animation:spin .8s linear infinite;margin:24px auto}}@keyframes spin{{to{{transform:rotate(360deg)}}}}@keyframes appear{{from{{opacity:0;transform:translateY(4px)}}to{{opacity:1;transform:none}}}}@media(prefers-reduced-motion:reduce){{main,.spinner{{animation:none}}}}</style><main aria-busy="true"><header><span>Q</span> QuantDSH</header><h1 role="status">{escaped}</h1><p>正在准备工作区并启动本机 DSH，请稍候。运行环境和内置插件已随安装包提供。</p><div class="spinner" aria-hidden="true"></div></main></html>"#
    )
}

/// Carrier initialization script: ports connection-preload.ts and routes
/// `quantdsh:` navigations to the Rust `quantdsh_navigate` command, mirroring the
/// Electron `will-navigate` interception for user gestures.
pub fn carrier_init_script(surface: &str) -> String {
    format!(
        r#"(() => {{
  'use strict';
  const QUANTDSH_SURFACE = {surface:?};
  const call = (command, args) => {{
    const internals = window.__TAURI_INTERNALS__;
    if (internals && typeof internals.invoke === 'function') return internals.invoke(command, args);
    const tauri = window.__TAURI__;
    if (tauri && typeof tauri.invoke === 'function') return tauri.invoke(command, args);
    return Promise.reject(new Error('tauri ipc unavailable'));
  }};
  const route = (url) => {{
    if (typeof url !== 'string' || !url.startsWith('quantdsh:')) return;
    void call('quantdsh_navigate', {{ url, surface: QUANTDSH_SURFACE }}).catch(() => {{}});
  }};
  document.addEventListener('click', (event) => {{
    const anchor = event.target instanceof Element ? event.target.closest('a[href^="quantdsh:"]') : null;
    if (!anchor) return;
    event.preventDefault();
    route(anchor.getAttribute('href'));
  }}, true);
  document.addEventListener('submit', (event) => {{
    const form = event.target;
    if (!(form instanceof HTMLFormElement)) return;
    const action = form.getAttribute('action') || '';
    if (!action.startsWith('quantdsh:')) return;
    event.preventDefault();
    const params = new URLSearchParams();
    for (const element of form.elements) {{
      if (!(element instanceof HTMLInputElement) || !element.name || element.disabled) continue;
      params.append(element.name, element.value);
    }}
    const query = params.toString();
    route(query ? action + '?' + query : action);
  }}, true);
  window.addEventListener('DOMContentLoaded', () => {{
    const form = document.querySelector('#enterprise-login');
    const status = document.querySelector('#status');
    if (!form || !status) return;
    let busy = false;
    const perform = async (value) => {{
      if (busy) return;
      busy = true; status.textContent = '正在核验企业账号并准备本机运行…';
      for (const button of form.querySelectorAll('button')) button.disabled = true;
      try {{
        const result = await call('quantdsh_enterprise_login', value);
        if (result && result.error) status.textContent = result.error;
        else status.textContent = '正在启动本机官方 DSH…';
      }} catch {{ status.textContent = '无法完成操作，请返回入口后重试' }}
      finally {{
        busy = false;
        for (const button of form.querySelectorAll('button')) button.disabled = false;
      }}
    }};
    form.addEventListener('submit', (event) => {{
      event.preventDefault();
      const account = document.querySelector('#account');
      const password = document.querySelector('#password');
      const value = {{ account: account.value, password: password.value }};
      password.value = '';
      void perform(value);
    }});
  }});
  window.addEventListener('DOMContentLoaded', () => {{
    const actions = document.querySelector('#runtime-actions');
    if (!actions) return;
    const status = document.querySelector('#runtime-status');
    const say = (text) => {{ if (status) status.textContent = text; }};
    let busy = false;
    actions.addEventListener('click', async (event) => {{
      const button = event.target instanceof Element ? event.target.closest('button[id]') : null;
      if (!button || busy) return;
      event.preventDefault();
      busy = true;
      for (const other of actions.querySelectorAll('button')) other.disabled = true;
      try {{
        if (button.id === 'runtime-import') {{
          say('正在选择运行时离线包…');
          const result = await call('quantdsh_runtime_import', {{}});
          if (result && result.cancelled) {{ say(''); return; }}
          if (result && result.error) {{ say(result.error); return; }}
          say('运行时已就绪，正在重启 QuantDSH…');
          await call('quantdsh_runtime_restart', {{}}).catch(() => {{}});
          return;
        }}
        if (button.id === 'runtime-download') {{
          const result = await call('quantdsh_runtime_open_download', {{}});
          say(result && result.error ? result.error : '已在浏览器中打开下载页');
          return;
        }}
        if (button.id === 'runtime-exit') {{
          say('正在退出…');
          await call('quantdsh_runtime_exit', {{}}).catch(() => {{}});
          return;
        }}
      }} catch {{
        say('操作失败，请重试');
      }} finally {{
        busy = false;
        for (const other of actions.querySelectorAll('button')) other.disabled = false;
      }}
    }});
  }});
}})();"#,
        surface = surface
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_page_matches_electron_template() {
        let html = connection_entry_html("https://example.com", false, true);
        assert!(html.starts_with("<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\">"));
        assert!(html.contains("form-action quantdsh:; base-uri 'none'"));
        assert!(html.contains("<a href=\"quantdsh://personal\">个人使用</a>"));
        assert!(html.contains("action=\"quantdsh://enterprise\" method=\"get\""));
        assert!(html.contains("value=\"https://example.com\""));
        assert!(html.contains("<button type=\"submit\">企业登录</button>"));
        assert!(html.ends_with("</html>"));
    }

    #[test]
    fn entry_page_escapes_portal() {
        let html = connection_entry_html("https://a\"b&c<d>e'f", false, true);
        assert!(html.contains("value=\"https://a&quot;b&amp;c&lt;d&gt;e&#39;f\""));
        assert!(!html.contains("a\"b&c<d>"));
    }

    #[test]
    fn managed_backend_hides_input() {
        let html = connection_entry_html("https://managed.example", true, false);
        assert!(html.contains("<label>企业后台</label><p>https://managed.example</p>"));
        assert!(!html.contains("id=\"portal\""));
        assert!(html.contains("请先安装企业账号插件"));
    }

    #[test]
    fn login_page_replaces_csp_once() {
        let html = enterprise_login_html("https://example.com");
        assert!(html.contains("form-action 'none'; base-uri 'none'"));
        assert!(html.contains("<form id=\"enterprise-login\">"));
        assert!(html.contains("登录企业 Desktop"));
        assert!(html.contains("<a href=\"quantdsh://entry\" style=\"margin-top:16px\">返回使用方式</a>"));
        // The base template's own form action must not survive.
        assert!(!html.contains("action=\"quantdsh://enterprise\""));
    }

    #[test]
    fn loading_page_keeps_spinner_and_message() {
        let html = connection_loading_html("正在启动个人工作区");
        assert!(html.contains("form-action quantdsh:"));
        assert!(html.contains("aria-busy=\"true\""));
        assert!(html.contains("正在启动个人工作区"));
        assert!(html.contains("class=\"spinner\""));
    }

    #[test]
    fn init_script_routes_quantdsh_urls() {
        let script = carrier_init_script("carrier");
        assert!(script.contains("QUANTDSH_SURFACE = \"carrier\""));
        assert!(script.contains("quantdsh_navigate"));
        assert!(script.contains("quantdsh_enterprise_login"));
        assert!(script.contains("#enterprise-login"));
        assert!(script.contains("quantdsh_runtime_import"));
        assert!(script.contains("#runtime-actions"));
    }

    #[test]
    fn runtime_page_names_the_failure_and_offers_repair() {
        let html = runtime_missing_html(
            "安装包内置运行时",
            "C:/Program Files/QuantDSH/resources/quantdsh-runtime",
            &[
                "缺少 Node.js 运行时：primary-runtime/dependencies/node/bin/node.exe".to_string(),
                "缺少 pnpm 安装工具：primary-runtime/dependencies/pnpm/bin/pnpm.cjs".to_string(),
            ],
            None,
            "C:/Users/example/AppData/Roaming/QuantDSH/runtime",
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("本机运行时不可用"));
        assert!(html.contains("安装包内置运行时 未通过完整性校验"));
        assert!(html.contains("缺少 Node.js 运行时"));
        assert!(html.contains("id=\"runtime-import\""));
        assert!(html.contains("未配置在线下载地址"));
        assert!(html.contains("id=\"runtime-exit\""));
        assert!(html.ends_with("</html>"));
    }

    #[test]
    fn runtime_page_offers_download_when_configured() {
        let html = runtime_missing_html(
            "已导入的运行时",
            "/opt/quantdsh",
            &["缺少 Python 运行时：x".to_string()],
            Some("https://example.com/runtime.tar.zst"),
            "/home/user/.quantdsh/runtime",
        );
        assert!(html.contains("id=\"runtime-download\""));
        assert!(!html.contains("未配置在线下载地址"));
    }

}
