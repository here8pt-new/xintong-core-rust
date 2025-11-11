// 用于处理 WebSocket 逻辑
// src/ws.rs（WebSocket 核心逻辑）
//这个文件将包含我们 websocket_endpoint 的 Rust 实现，它完美复刻了 Python 版 Core 的所有逻辑（注册、心跳、路由）。

// src/ws.rs (最终心跳修复版)

// src/ws.rs (最终修复版 - 修复所有权)

// src/ws.rs (已添加 Auth)

use crate::state::{AppState, LocalConnectionMap};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::headers::{authorization::Bearer, Authorization};
use axum_extra::TypedHeader;
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::pin;
use tokio::time::{self, Duration};
use tracing::{info, warn};

// --- 导入我们的 auth 模块 ---
use crate::auth;

// --- 1. 修复: 填上完整的 JSON 结构体定义 ---

#[derive(Deserialize, Debug)]
struct RegisterMessage {
    r#type: String,
    agent_id: String,
    capabilities: Vec<String>,
}

#[derive(Serialize, Debug)]
struct RegisterAck {
    r#type: &'static str,
    status: &'static str,
    message: String,
}

// --- 2. 主 WebSocket Handler (已添加 Auth) ---

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    TypedHeader(auth_header): TypedHeader<Authorization<Bearer>>,
) -> Response {
    // 5. --- 执行认证 ---
    let token = auth_header.token();
    if auth::validate_token(token, &agent_id).is_err() {
        warn!("[Auth] Agent '{}' 提供了无效的 Token。拒绝连接。", agent_id);
        return (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response();
    }
    // --- 认证通过 ---
    info!("[Auth] Agent '{}' Token 验证通过。", agent_id);
    
    // 6. 认证通过后，才升级为 WebSocket
    ws.on_upgrade(move |socket| handle_socket(socket, state, agent_id))
}

// --- 3. handle_socket 及其所有子函数 (保持不变) ---

async fn handle_socket(socket: WebSocket, state: AppState, agent_id: String) {
    let (sink, stream) = socket.split();

    // 1. 注册本地连接
    if !register_connection(sink, &state.local_connections, &agent_id).await {
        return;
    }

    // 2. 等待并处理注册消息
    let reg_stream = match handle_registration(stream, &state, &agent_id).await {
        Ok(s) => s,
        Err(_) => {
            if let Some(mut sink) = state.local_connections.lock().await.remove(&agent_id) {
                let _ = sink.close().await;
            }
            return;
        }
    };

    // 3. 创建两个并行的任务
    let state_clone = state.clone();
    let agent_id_clone = agent_id.clone();
    
    let heartbeat_task = tokio::spawn(async move {
        run_heartbeat_loop(state_clone, agent_id_clone).await;
    });

    let state_for_receive = state.clone();
    let agent_id_for_receive = agent_id.clone();

    let receive_task = tokio::spawn(async move {
        run_receive_loop(reg_stream, state_for_receive, agent_id_for_receive).await;
    });
    
    pin!(heartbeat_task);
    pin!(receive_task);

    tokio::select! {
        _ = &mut heartbeat_task => {
            warn!("[{}] 心跳任务已终止。", agent_id);
            receive_task.abort();
        },
        _ = &mut receive_task => {
            info!("[{}] 接收任务已终止 (客户端断开)。", agent_id);
            heartbeat_task.abort();
        },
    }

    info!("[{}] 正在清理...", agent_id);
    state.local_connections.lock().await.remove(&agent_id);
    state.unregister_agent(&agent_id).await;
}

async fn register_connection(
    sink: SplitSink<WebSocket, Message>,
    local_connections: &LocalConnectionMap,
    agent_id: &str,
) -> bool {
    if local_connections.lock().await.contains_key(agent_id) {
        warn!("[{}] 连接被拒绝: Agent ID 已在此实例上连接。", agent_id);
        return false;
    }
    
    local_connections
        .lock()
        .await
        .insert(agent_id.to_string(), sink);
        
    info!("[Core-{}] Agent '{}' 本地连接。", &"Rust"[..4], agent_id);
    true
}

async fn handle_registration(
    mut stream: SplitStream<WebSocket>,
    state: &AppState,
    agent_id: &str,
) -> Result<SplitStream<WebSocket>, ()> {
    if let Some(Ok(Message::Text(msg_text))) = stream.next().await {
        match serde_json::from_str::<RegisterMessage>(&msg_text) {
            // --- 修复: 现在 reg_msg.r#type, .agent_id, .capabilities 都可用了 ---
            Ok(reg_msg) if reg_msg.r#type == "register" && reg_msg.agent_id == agent_id => {
                state.register_agent(agent_id, &reg_msg.capabilities).await;

                let ack = RegisterAck {
                    r#type: "register_ack",
                    status: "success",
                    message: format!("Agent '{}' registered.", agent_id),
                };
                if let Some(sink) = state.local_connections.lock().await.get_mut(agent_id) {
                   if sink.send(Message::Text(serde_json::to_string(&ack).unwrap())).await.is_err() {
                        warn!("[{}] 发送 register_ack 失败。", agent_id);
                        return Err(());
                   }
                }
                return Ok(stream);
            }
            _ => {
                warn!("[{}] 第一条消息不是有效的 'register'。断开连接。", agent_id);
                return Err(());
            }
        }
    }
    
    warn!("[{}] 未收到注册消息。断开连接。", agent_id);
    Err(())
}


async fn run_heartbeat_loop(state: AppState, agent_id: String) {
    let mut interval = time::interval(Duration::from_secs(30));
    interval.tick().await; 
    
    loop {
        interval.tick().await;
        info!("[{}] 正在刷新心跳...", agent_id);
        state.keep_alive(&agent_id).await;
    }
}

async fn run_receive_loop(
    mut stream: SplitStream<WebSocket>,
    state: AppState,
    agent_id: String,
) {
    while let Some(msg_option) = stream.next().await {
        match msg_option {
            Ok(Message::Text(msg_text)) => {
                if let Ok(data) = serde_json::from_str::<Value>(&msg_text) {
                    if let Some(target_id) = data["target_agent_id"].as_str() {
                        
                        let target_id_str = target_id.to_string();
                        
                        if let Some(location) = state.find_agent_location(&target_id_str).await {
                            let channel = format!("xintong:core:{}", location);
                            info!("[{}] 正在路由消息给 {} (在 {})", agent_id, target_id_str, &location[..8]);
                            state.publish_message(&channel, &msg_text).await;
                        } else {
                            info!("[{}] 目标 Agent '{}' 未找到。", agent_id, target_id_str);
                            let err_resp = json!({
                                "type": "response",
                                "correlation_id": data["message_id"].as_str().unwrap_or("unknown"),
                                "source_agent_id": "xintong-core",
                                "target_agent_id": agent_id,
                                "status": 404,
                                "payload": {"error": format!("Target agent '{}' not found.", target_id_str)}
                            });
                            if let Some(sink) = state.local_connections.lock().await.get_mut(&agent_id) {
                                if sink.send(Message::Text(err_resp.to_string())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Ok(Message::Close(_)) | Err(_) => {
                break;
            }
            _ => { }
        }
    }
}