# ccm — Claude Code 多环境管理器

极速、安全的 Claude Code 多环境管理器。把多套 Claude Code 环境配置（不同 API/中转地址、Token、模型）**加密**保存在本地，运行时按需 **JIT 解密**并通过 `exec` 进程替换直接拉起官方 `claude`，从而在零内存驻留、完美继承官方全局 Threads 与 MCP 的前提下安全地多环境切换。

> 平台：Unix（macOS / Linux）。依赖 `std::os::unix` 的权限控制与 `exec` 能力。

## 工作原理

```text
[ 1. 静态加密存储 ]            [ 2. JIT 解密 ]                [ 3. 进程接管 ]
~/.config/ccm/                读取并匹配选中的 profile        execvp:
 ├── .master_key  (0600)  ->  用 .master_key 解密         ->  claude --settings <file>
 └── profiles.json            生成 settings/<id>.json (0600)   claude 覆盖 ccm 进程
     (encrypted_settings)     (含明文 settings)               (ccm 不驻留内存)
```

- **加密**：根密钥 `.master_key`（32 字节随机，AES-256-GCM）以 `0600` 权限存放；每个环境的整份 settings JSON 加密为 `encrypted_settings` 落入 `profiles.json`，磁盘上**不含明文 Token**。
- **JIT**：启动时才把选中环境解密为 `settings/<id>.json`（同样强制 `0600`），随即交给 `claude`。
- **进程接管**：用 `exec` 让 `claude` 从底层覆盖 `ccm` 进程，stdin/stdout/stderr 完美继承，无僵尸进程。

## 安装

```bash
cargo build --release
# 产物：target/release/ccm
cp target/release/ccm /usr/local/bin/   # 或加入你的 PATH
```

## 使用

### 添加环境 `ccm add`

支持两种方式：

**命令行（非交互，可脚本化）** —— 同时给出 `--name` 与 `--token` 即直接添加：

```bash
ccm add --name deepseek \
        --token sk-xxxxxxxx \
        --base-url https://api.deepseek.com/anthropic \
        --model deepseek-chat
```

| 参数 | 必填 | 说明 |
|---|---|---|
| `--name` | 是 | 环境名称，也是模糊匹配键 |
| `--token` | 是 | 认证 Token，写入 `env.ANTHROPIC_AUTH_TOKEN` |
| `--base-url` | 否 | 中转/自建端点，写入 `env.ANTHROPIC_BASE_URL`；留空用官方端点 |
| `--model` | 否 | 默认模型，写入 `env.ANTHROPIC_MODEL` |

**人机交互（inquire 表单）** —— 省略参数即逐项提示，Token 以掩码方式输入：

```bash
ccm add
```

同名环境（不区分大小写）会被覆盖。

### 列出环境 `ccm list`

```bash
ccm list
```

打印全部可用环境（name / id / 来源），**不显示明文 Token**。来源含本地添加（`local`）与从 cc-switch 读取（`cc-switch`）。

### 启动环境

```bash
ccm deepseek      # 按 id/name 模糊匹配，命中唯一则直接启动
ccm               # 无参：唤起交互式列表，方向键/输入文本过滤后回车启动
```

匹配成功后即解密生成 JIT 配置并 `exec` 拉起 `claude`。

## cc-switch 兼容

若检测到 `~/.cc-switch/cc-switch.db`，ccm 会自动读取其 `providers` 表中 `app_type = 'claude'` 的全部环境（取每个 provider 完整的 `settings_config` 原样承载，保留其原始 env 变量名），与本地环境按 name 去重合并（本地优先）。

- cc-switch 数据**只读不回写**，不会修改你的 cc-switch 配置。
- 数据库不存在、表缺失或某条记录解析失败时静默跳过，不影响其它命令。

## 文件布局

```
~/.config/ccm/
 ├── .master_key          # AES-256 根密钥 (0600)
 ├── profiles.json        # 加密后的本地环境
 └── settings/
     └── <id>.json        # JIT 运行时生成的明文 settings (0600)
```

> 目录遵循 XDG：优先 `$XDG_CONFIG_HOME`，否则 `$HOME/.config`。

## 安全说明

- 静态存储仅含密文；明文 Token 只在启动瞬间出现在 `0600` 权限的 JIT 文件里。
- `.master_key` 一旦丢失或更换，已有 `encrypted_settings` 将无法解密。
- JIT 文件包含明文 Token，请勿将 `~/.config/ccm/settings/` 纳入版本控制或共享。

## 开发

```bash
cargo test                              # 单元测试
cargo clippy --all-targets -- -D warnings
cargo build --release
```

## License

GPL-3.0-only
