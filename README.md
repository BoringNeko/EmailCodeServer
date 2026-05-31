# EmailServer

邮箱验证码中继服务器，负责连接邮箱、提取验证码并通过 WebSocket 推送给客户端。

**版本**: V1.0.0_20260531_Alpha

## 功能

- 连接多个邮箱（IMAP IDLE 实时推送 / POP3 定时轮询）
- 自动提取验证码（正则优先，可选 LLM 兜底）
- WebSocket 向已认证客户端广播验证码
- Docker 运行，最小镜像 < 25MB

## 快速开始

### 1. 准备配置

```bash
cp config.example.yaml config.yaml
```

编辑 `config.yaml`：

```yaml
bind: "0.0.0.0:8080"
api_keys:
  - "your-secret-key"

accounts:
  - name: "my-email"
    protocol: "imap"          # imap | pop3
    host: "imap.example.com"
    port: 993
    tls: true
    email: "user@example.com"
    password: "app-password"
```

### 2. 运行

```bash
# 本地
cargo run --release -- config.yaml

# Docker
docker compose up -d
```

### 3. 环境变量覆盖

```bash
# Windows
set EMAILSERVER_BIND=0.0.0.0:9090
set EMAILSERVER_API_KEYS=key1,key2
set RUST_LOG=debug
email-server.exe config.yaml
```

## 验证码提取

默认正则（按优先级）：

| 模式 | 示例 |
|------|------|
| `\b\d{6}\b` | 123456 |
| `\b\d{4}-\d{4}\b` | 1234-5678 |
| `\b\d{8}\b` | 12345678 |

自定义正则：

```yaml
extraction:
  patterns:
    - '\b\d{6}\b'
    - '\b[A-Z0-9]{8}\b'
```

LLM 兜底（正则全部失败时启用）：

```yaml
extraction:
  llm_fallback:
    enabled: true
    provider: "openai"
    api_key: "sk-xxx"
    model: "gpt-4.1-mini"
    base_url: "https://api.openai.com/v1"
```

编译时禁用 LLM（减小体积，4.2MB → 2.9MB）：

```bash
cargo build --release --no-default-features
```

## WebSocket 协议

```
客户端 → 服务端:
  {"type":"auth","api_key":"xxx"}
  {"type":"ping"}

服务端 → 客户端:
  {"type":"auth_ok"}
  {"type":"auth_fail","reason":"invalid api_key"}
  {"type":"new_code","code":"123456","subject":"...","from":"...","time":"...","account":"..."}
  {"type":"pong"}
```

## 构建要求

- Rust 1.80+
- Docker（可选）

## 依赖许可

所有依赖均为开源协议，无闭源组件：

| crate | 协议 |
|-------|------|
| tokio, tokio-tungstenite, tokio-util, tracing, tracing-subscriber | MIT |
| async-imap, async-native-tls, futures-util | MIT OR Apache-2.0 |
| regex, serde, serde_json, serde_yaml, mailparse | MIT OR Apache-2.0 |
| chrono, anyhow | MIT OR Apache-2.0 |
| reqwest (可选 LLM 功能) | MIT OR Apache-2.0 |

## 许可证

Apache-2.0

## 注意事项

使用 deepseek-v4-pro + claude code 开发<br>
许多功能暂未完善<br>
