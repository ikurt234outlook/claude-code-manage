//! UI 层：inquire 交互（add 表单、无参 Select 模糊选择）。

use anyhow::{anyhow, Context, Result};
use inquire::{Password, PasswordDisplayMode, Select, Text};

use crate::config::{self, Profile, Source};
use crate::crypto;
use crate::engine;

/// 非交互添加：核心落盘逻辑。`ccm add --name <> --token <>` 与交互表单都复用它。
///
/// `base_url` / `model` 为空字符串表示省略对应 env 字段。
/// 同名（不区分大小写）的本地 Profile 会被覆盖；cc-switch 源不受影响。
pub fn add_noninteractive(
    master_key: &[u8; 32],
    name: &str,
    auth_token: &str,
    base_url: &str,
    model: &str,
) -> Result<Profile> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("name 不能为空"));
    }
    if auth_token.trim().is_empty() {
        return Err(anyhow!("认证 Token 不能为空"));
    }

    let settings = engine::build_settings_json(auth_token.trim(), base_url.trim(), model.trim());
    let settings_text =
        serde_json::to_string_pretty(&settings).context("序列化 settings JSON 失败")?;

    let mut existing = config::load_profiles(master_key)?;
    let id = config::derive_unique_id(name, &existing);
    let encrypted_settings = crypto::encrypt(&settings_text, master_key)?;
    let profile = Profile {
        id,
        name: name.to_string(),
        encrypted_settings,
        source: Source::Local,
    };

    // 同名覆盖（不区分大小写）：仅在本地集合内替换，再仅持久化 Local 源。
    existing.retain(|p| !(p.source == Source::Local && p.name.eq_ignore_ascii_case(&profile.name)));
    existing.push(profile.clone());
    config::save_profiles(&existing)?;
    Ok(profile)
}

/// `ccm add` 交互流程：inquire 表单收集输入 → 复用 [`add_noninteractive`] 落盘。
pub fn add_flow(master_key: &[u8; 32]) -> Result<Profile> {
    let name = Text::new("环境名称 (name):")
        .with_help_message("将作为模糊匹配键；唯一标识由 name 归一化得到")
        .prompt()
        .context("读取 name 失败")?;

    let base_url = Text::new("Base URL (可空):")
        .with_default("")
        .with_help_message("如 https://api.deepseek.com/anthropic；留空使用 Claude 官方端点")
        .prompt()
        .context("读取 base_url 失败")?;

    let model = Text::new("模型 (model, 可空):")
        .with_default("")
        .with_help_message("写入 env.ANTHROPIC_MODEL；如 claude-opus-4-8；留空则不写入")
        .prompt()
        .context("读取 model 失败")?;

    let auth_token = Password::new("认证 Token (ANTHROPIC_AUTH_TOKEN):")
        .with_display_mode(PasswordDisplayMode::Masked)
        .with_help_message("将整份 settings 经 AES-256-GCM 加密后落盘，不会以明文出现在 profiles.json")
        .without_confirmation()
        .prompt()
        .context("读取认证 Token 失败")?;

    add_noninteractive(master_key, &name, &auth_token, &base_url, &model)
}

/// 无参 `ccm`：交互式 Select。inquire 默认支持方向键 + 输入文本模糊过滤。
pub fn select_flow(profiles: &[Profile]) -> Result<Profile> {
    if profiles.is_empty() {
        return Err(anyhow!(
            "没有可用环境。请先运行 `ccm add` 添加，或确认 cc-switch 数据可访问。"
        ));
    }
    let options: Vec<String> = profiles.iter().map(format_option).collect();
    let chosen = Select::new("选择要启动的 Claude Code 环境:", options.clone())
        .with_help_message("方向键移动 / 直接输入文本进行模糊过滤 / 回车确认")
        .prompt()
        .context("交互选择被取消")?;
    let idx = options
        .iter()
        .position(|s| s == &chosen)
        .ok_or_else(|| anyhow!("内部错误：选项与 profile 索引不一致"))?;
    Ok(profiles[idx].clone())
}

fn source_tag(s: Source) -> &'static str {
    match s {
        Source::Local => "local",
        Source::CcSwitch => "cc-switch",
    }
}

fn format_option(p: &Profile) -> String {
    format!("{}  ({}, {})", p.name, p.id, source_tag(p.source))
}

/// 模糊匹配：query 对 id / name 不区分大小写做子串匹配。
/// 精确命中 id/name 优先；唯一子串命中次之；多命中或零命中返回错误。
pub fn fuzzy_pick<'a>(profiles: &'a [Profile], query: &str) -> Result<&'a Profile> {
    let q = query.to_lowercase();
    if let Some(p) = profiles
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(query) || p.name.eq_ignore_ascii_case(query))
    {
        return Ok(p);
    }
    let hits: Vec<&Profile> = profiles
        .iter()
        .filter(|p| p.id.to_lowercase().contains(&q) || p.name.to_lowercase().contains(&q))
        .collect();
    match hits.len() {
        0 => Err(anyhow!("未找到匹配环境: {query}")),
        1 => Ok(hits[0]),
        _ => {
            let names: Vec<&str> = hits.iter().map(|p| p.name.as_str()).collect();
            Err(anyhow!(
                "查询 `{query}` 匹配到多个环境: {}；请提供更精确的名称",
                names.join(", ")
            ))
        }
    }
}

/// 打印列表。仅显示 name / id / source，绝不解密或显示明文 token。
pub fn print_list(profiles: &[Profile]) {
    if profiles.is_empty() {
        println!("(空) 暂无环境配置。运行 `ccm add` 添加。");
        return;
    }
    println!("共 {} 个环境:", profiles.len());
    for p in profiles {
        println!(
            "  - {:<28}  id={:<28}  source={}",
            p.name,
            p.id,
            source_tag(p.source)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: &str, name: &str) -> Profile {
        Profile {
            id: id.into(),
            name: name.into(),
            encrypted_settings: "x".into(),
            source: Source::Local,
        }
    }

    #[test]
    fn exact_match_wins_over_substring() {
        let ps = vec![mk("deepseek", "DeepSeek"), mk("deepseek-pro", "DeepSeek Pro")];
        let p = fuzzy_pick(&ps, "deepseek").unwrap();
        assert_eq!(p.id, "deepseek");
    }

    #[test]
    fn unique_substring_match() {
        let ps = vec![mk("deepseek", "DeepSeek"), mk("kimi", "Kimi K2")];
        let p = fuzzy_pick(&ps, "ki").unwrap();
        assert_eq!(p.id, "kimi");
    }

    #[test]
    fn ambiguous_substring_errors() {
        let ps = vec![mk("a-pro", "A Pro"), mk("b-pro", "B Pro")];
        let err = fuzzy_pick(&ps, "pro").unwrap_err().to_string();
        assert!(err.contains("匹配到多个环境"));
    }

    #[test]
    fn no_match_errors() {
        let ps = vec![mk("a", "A")];
        let err = fuzzy_pick(&ps, "zzz").unwrap_err().to_string();
        assert!(err.contains("未找到匹配环境"));
    }
}
