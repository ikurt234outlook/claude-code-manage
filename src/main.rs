//! ccm — Claude Code 多环境管理器入口。
//!
//! 路由：
//!   ccm add               -> 交互式录入并加密存档
//!   ccm list              -> 列出全部环境（JSON + cc-switch SQLite 合并去重）
//!   ccm <id|name>         -> 模糊匹配后 JIT 解密并 exec 拉起 claude
//!   ccm                   -> inquire Select 交互选择后 exec
//!
//! 顶层用 anyhow::Result 汇聚错误；exec 成功不返回，failure 由 anyhow 打印。

mod config;
mod crypto;
mod engine;
mod paths;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "ccm",
    bin_name = "ccm",
    about = "Claude Code 多环境管理器：加密存储 + JIT 解密 + exec 进程接管",
    arg_required_else_help = false,
    disable_help_subcommand = true
)]
struct Cli {
    /// 子命令；省略且无 query 则进入交互式选择。
    #[command(subcommand)]
    cmd: Option<Cmd>,

    /// 当未指定子命令时：作为 profile 的模糊查询（id 或 name）。
    /// 例：`ccm deepseek`。
    #[arg()]
    query: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 添加新环境（整份 settings 加密落盘）。
    ///
    /// 两种用法：
    ///   - 非交互：同时给出 `--name` 与 `--token` 即直接添加（可脚本化）。
    ///   - 交互：省略必填项则唤起 inquire 表单逐项补齐。
    Add {
        /// 环境名称（模糊匹配键）。与 --token 同时给出则走非交互模式。
        #[arg(long)]
        name: Option<String>,
        /// 认证 Token，写入 env.ANTHROPIC_AUTH_TOKEN。
        #[arg(long)]
        token: Option<String>,
        /// Base URL（可空），写入 env.ANTHROPIC_BASE_URL。
        #[arg(long)]
        base_url: Option<String>,
        /// 模型（可空），写入 env.ANTHROPIC_MODEL。
        #[arg(long)]
        model: Option<String>,
    },
    /// 列出全部环境
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // 所有命令都需要核心目录与 master key。
    paths::ensure_dirs()?;
    let master_key = crypto::load_or_create_master_key()?;

    match (cli.cmd, cli.query) {
        (
            Some(Cmd::Add {
                name,
                token,
                base_url,
                model,
            }),
            _,
        ) => {
            let profile = match (name, token) {
                // 非交互：name + token 齐全，直接添加（可脚本化）。
                (Some(name), Some(token)) => ui::add_noninteractive(
                    &master_key,
                    &name,
                    &token,
                    base_url.as_deref().unwrap_or(""),
                    model.as_deref().unwrap_or(""),
                )?,
                // 否则进入交互表单（已给出的项作为默认值预填）。
                _ => ui::add_flow(&master_key)?,
            };
            println!("已添加环境: {} (id={})", profile.name, profile.id);
            Ok(())
        }
        (Some(Cmd::List), _) => {
            let profiles = config::load_profiles(&master_key)?;
            ui::print_list(&profiles);
            Ok(())
        }
        (None, Some(query)) => {
            let profiles = config::load_profiles(&master_key)?;
            let target = ui::fuzzy_pick(&profiles, &query)?.clone();
            engine::launch(&target, &master_key)
        }
        (None, None) => {
            let profiles = config::load_profiles(&master_key)?;
            let target = ui::select_flow(&profiles)?;
            engine::launch(&target, &master_key)
        }
    }
}
