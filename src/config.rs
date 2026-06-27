//! 数据层：Profile 模型、profiles.json 读写、cc-switch 兼容读取、聚合去重。
//!
//! 设计要点（基于对真实 cc-switch 数据库的实测）：
//! - cc-switch 每个 provider 的 `settings_config` 本身就是一份**完整可用**的
//!   claude settings JSON（含 `env.ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_BASE_URL`
//!   / `ANTHROPIC_MODEL` 等）。因此 ccm 不拆字段，而是整份承载 settings JSON，
//!   加密后存为 `encrypted_settings`，启动时原样还原，100% 复刻连接行为。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::paths;

/// Profile 来源标记，便于 `ccm list` 区分本地与 cc-switch 导入。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// 本地 `ccm add` 创建，写入 profiles.json。
    #[default]
    Local,
    /// 从 `~/.cc-switch/cc-switch.db` 读取的兼容数据（不回写）。
    CcSwitch,
}

/// 单个环境配置。
///
/// `encrypted_settings` 为 [`crate::crypto::encrypt`] 的输出（base64），其明文
/// 是一份完整的 claude settings JSON 字符串。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub encrypted_settings: String,
    #[serde(default)]
    pub source: Source,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct ProfilesFile {
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

/// 从 `name` 派生 id：小写、非 `[a-z0-9-_]` 字符转 `-`，挤掉连续 `-`，去掉首尾 `-`。
pub fn normalize_id(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.chars() {
        let mapped = ch.to_ascii_lowercase();
        let ok = mapped.is_ascii_alphanumeric() || mapped == '-' || mapped == '_';
        if ok {
            out.push(mapped);
            last_dash = mapped == '-';
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    // 去掉首部 `-`（如原始 id 以中文/符号开头被归一化所致）。
    let trimmed = out.trim_start_matches('-');
    let mut out = if trimmed.len() == out.len() {
        out
    } else {
        trimmed.to_string()
    };
    if out.is_empty() {
        out.push_str("profile");
    }
    out
}

/// 在已有 id 集合中分配一个不冲突的 id；冲突则追加 `-2`、`-3` …
pub fn allocate_id(base: &str, taken: &[&str]) -> String {
    if !taken.contains(&base) {
        return base.to_string();
    }
    let mut n = 2usize;
    loop {
        let cand = format!("{base}-{n}");
        if !taken.contains(&cand.as_str()) {
            return cand;
        }
        n += 1;
    }
}

/// 由 name 派生一个在当前 profile 集合中不冲突的 id。
pub fn derive_unique_id(name: &str, existing: &[Profile]) -> String {
    let base = normalize_id(name);
    let taken: Vec<&str> = existing.iter().map(|p| p.id.as_str()).collect();
    allocate_id(&base, &taken)
}

/// 读取 JSON 主数据源；不存在视为空。
pub fn load_json() -> Result<Vec<Profile>> {
    let path = paths::profiles_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(&path)
        .with_context(|| format!("读取 profiles.json 失败: {}", path.display()))?;
    if data.trim().is_empty() {
        return Ok(Vec::new());
    }
    let file: ProfilesFile = serde_json::from_str(&data)
        .with_context(|| format!("解析 profiles.json 失败: {}", path.display()))?;
    Ok(file.profiles)
}

/// 序列化写回 JSON（pretty）。仅持久化 `source == Local` 的 Profile。
pub fn save_profiles(profiles: &[Profile]) -> Result<()> {
    let path = paths::profiles_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
    }
    let local: Vec<Profile> = profiles
        .iter()
        .filter(|p| p.source == Source::Local)
        .cloned()
        .collect();
    let file = ProfilesFile { profiles: local };
    let s = serde_json::to_string_pretty(&file).context("序列化 profiles.json 失败")?;
    fs::write(&path, s).with_context(|| format!("写入 profiles.json 失败: {}", path.display()))?;
    Ok(())
}

/// best-effort：尝试读取 `~/.cc-switch/cc-switch.db` 的 `providers` 表
/// （`app_type = 'claude'`）。
///
/// 任何错误（文件不存在、表缺失、列缺失、JSON 非法）一律返回空 vec，**不**向上传播。
fn load_from_cc_switch(master_key: &[u8; 32]) -> Vec<Profile> {
    let path = match paths::cc_switch_db() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    load_from_cc_switch_at(&path, master_key).unwrap_or_default()
}

fn load_from_cc_switch_at(path: &Path, master_key: &[u8; 32]) -> Result<Vec<Profile>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .context("打开 cc-switch sqlite 失败")?;

    let mut stmt = conn.prepare(
        "SELECT id, name, settings_config FROM providers WHERE app_type = 'claude'",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;

    let mut out = Vec::new();
    let mut taken: Vec<String> = Vec::new();
    for (raw_id, name, settings_config) in rows.flatten() {
        // settings_config 必须是合法 JSON（即 claude settings），否则跳过该行。
        if serde_json::from_str::<serde_json::Value>(&settings_config).is_err() {
            continue;
        }
        if name.trim().is_empty() {
            continue;
        }
        let encrypted_settings = match crate::crypto::encrypt(&settings_config, master_key) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let base = if raw_id.trim().is_empty() {
            normalize_id(&name)
        } else {
            normalize_id(&raw_id)
        };
        let refs: Vec<&str> = taken.iter().map(|s| s.as_str()).collect();
        let id = allocate_id(&base, &refs);
        taken.push(id.clone());
        out.push(Profile {
            id,
            name,
            encrypted_settings,
            source: Source::CcSwitch,
        });
    }
    Ok(out)
}

/// 聚合：JSON 优先，cc-switch 仅补充未出现的 name（不区分大小写）。
pub fn merge(json_src: Vec<Profile>, cc_src: Vec<Profile>) -> Vec<Profile> {
    let mut seen: std::collections::HashSet<String> =
        json_src.iter().map(|p| p.name.to_lowercase()).collect();
    let mut taken_ids: std::collections::HashSet<String> =
        json_src.iter().map(|p| p.id.clone()).collect();
    let mut out = json_src;
    for mut p in cc_src {
        let lower = p.name.to_lowercase();
        if seen.contains(&lower) {
            continue;
        }
        if taken_ids.contains(&p.id) {
            let refs: Vec<&str> = taken_ids.iter().map(|s| s.as_str()).collect();
            p.id = allocate_id(&p.id, &refs);
        }
        taken_ids.insert(p.id.clone());
        seen.insert(lower);
        out.push(p);
    }
    out
}

/// 主入口：合并 JSON + cc-switch fallback。
///
/// 需要 master_key：cc-switch 的明文 settings 在读入后立即加密为 `encrypted_settings`，
/// 内存中不长期保留明文。
pub fn load_profiles(master_key: &[u8; 32]) -> Result<Vec<Profile>> {
    let json_src = load_json()?;
    let cc_src = load_from_cc_switch(master_key);
    Ok(merge(json_src, cc_src))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_basic() {
        assert_eq!(normalize_id("DeepSeek"), "deepseek");
        assert_eq!(normalize_id("My Env 01"), "my-env-01");
        assert_eq!(normalize_id("  spaces  "), "spaces");
        assert_eq!(normalize_id("a/b/c"), "a-b-c");
        assert_eq!(normalize_id("---"), "profile");
        // 原始 id 以中文/符号开头：归一化后不应残留前导 `-`。
        assert_eq!(normalize_id("小公益站-1776044292015"), "1776044292015");
        assert_eq!(normalize_id("中文名"), "profile");
    }

    #[test]
    fn allocate_id_no_conflict() {
        assert_eq!(allocate_id("foo", &["bar"]), "foo");
    }

    #[test]
    fn allocate_id_with_conflict() {
        assert_eq!(allocate_id("foo", &["foo"]), "foo-2");
        assert_eq!(allocate_id("foo", &["foo", "foo-2"]), "foo-3");
    }

    fn mk(id: &str, name: &str, source: Source) -> Profile {
        Profile {
            id: id.into(),
            name: name.into(),
            encrypted_settings: "x".into(),
            source,
        }
    }

    #[test]
    fn merge_dedup_by_name_case_insensitive() {
        let j = vec![mk("a", "Alpha", Source::Local)];
        let s = vec![
            mk("a2", "alpha", Source::CcSwitch),
            mk("b", "Beta", Source::CcSwitch),
        ];
        let merged = merge(j, s);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "Alpha"); // JSON 优先
        assert_eq!(merged[1].name, "Beta");
    }

    #[test]
    fn merge_resolves_id_conflict() {
        let j = vec![mk("dup", "A", Source::Local)];
        let s = vec![mk("dup", "B", Source::CcSwitch)];
        let merged = merge(j, s);
        assert_eq!(merged.len(), 2);
        assert_ne!(merged[0].id, merged[1].id);
    }

    #[test]
    fn save_profiles_excludes_cc_switch() {
        // 纯逻辑校验：过滤规则只保留 Local。
        let all = [
            mk("a", "A", Source::Local),
            mk("b", "B", Source::CcSwitch),
        ];
        let local: Vec<&Profile> = all.iter().filter(|p| p.source == Source::Local).collect();
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].id, "a");
    }
}
