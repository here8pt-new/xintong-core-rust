// src/main.rs (修复版)

// src/main.rs (修复版)

mod state;
mod ws;
mod auth; // <-- 1. 添加这一行，声明 auth.rs

use axum::{
    extract::{ws::Message, DefaultBodyLimit},
    routing::get,
    Router,
};
use deadpool_redis::{Config, Runtime};
use deadpool_redis::redis;
use futures_util::SinkExt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tracing::{error, info};

use crate::state::{AppState, LocalConnectionMap};
use crate::ws::websocket_handler;

// --- Pub/Sub 监听器 ---
async fn pubsub_listener(
    client: redis::Client,
    core_instance_id: String,
    local_connections: LocalConnectionMap,
) {
    let channel_name = format!("xintong:core:{}", core_instance_id);
    info!("[PubSub] 正在连接到 Redis PubSub...");

    let mut redis_conn = match client.get_async_connection().await {
        Ok(conn) => conn.into_pubsub(),
        Err(e) => {
            error!("[PubSub] 无法获取 PubSub 连接: {}", e);
            return;
        }
    };

    if let Err(e) = redis_conn.subscribe(&channel_name).await {
        error!("[PubSub] 无法订阅频道 {}: {}", channel_name, e);
        return;
    }

    info!("[PubSub] 成功订阅频道: '{}'", channel_name);

    let mut stream = redis_conn.on_message();
    while let Some(msg) = stream.next().await {
        let payload: String = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                error!("[PubSub] 无法解析 payload: {}", e);
                continue;
            }
        };
        
        info!("[PubSub] 收到消息: {}", &payload[..60.min(payload.len())]);

        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&payload) {
            if let Some(target_id) = data["target_agent_id"].as_str() {
                
                let mut conns = local_connections.lock().await;
                
                if let Some(sink) = conns.get_mut(target_id) {
                    if let Err(e) = sink.send(Message::Text(payload)).await {
                        error!("[PubSub] 转发给 '{}' 失败: {}", target_id, e);
                    } else {
                        info!("[PubSub] 已转发消息给本地 Agent '{}'", target_id);
                    }
                } else {
                    error!("[PubSub] 错误: Agent '{}' 在 Redis 中指向我，但我没有它的本地连接。", target_id);
                }
            }
        }
    }
    
    info!("[PubSub] 监听器已停止。");
}

// --- 主函数 (服务器入口) ---
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("info,tower_http=debug,axum::rejection=trace")
        .init();

    info!("启动 Xintong Rust Core (M1)...");

    let redis_url = "redis://localhost:6379";
    let client = redis::Client::open(redis_url)
        .expect("无效的 Redis URL");
    let cfg = Config::from_url(redis_url);
    let pool = cfg.create_pool(Some(Runtime::Tokio1))
        .expect("无法创建 Redis 连接池");
    
    info!("[Redis] Redis 连接池已创建。");

    match pool.get().await {
        Ok(mut conn) => {
            let pong: String = redis::cmd("PING") 
                .query_async(&mut *conn).await
                .expect("Redis PING 失败");
            info!("[Redis] 成功连接到 Redis (PING -> {})", pong);
        }
        Err(e) => {
            error!("[Redis] 严重错误: 无法连接到 Redis。 {}. 请确保 Docker Redis 正在运行。", e);
            return; 
        }
    }

    let core_id = format!("core-instance:{}", uuid::Uuid::new_v4());
    let local_conns = Arc::new(Mutex::new(HashMap::new()));
    
    let state = AppState {
        redis_pool: pool.clone(),
        // redis_client: client.clone(), // <-- 修复: 移除
        core_instance_id: core_id.clone(),
        local_connections: local_conns.clone(),
    };

    tokio::spawn(pubsub_listener(client, core_id.clone(), local_conns)); 

    let app = Router::new()
        .route("/", get(handler_root))
        .route("/ws/:agent_id", get(websocket_handler))
        .with_state(state)
        .layer(DefaultBodyLimit::max(1024 * 1024 * 10));

    let addr = SocketAddr::from(([127, 0, 0, 1], 8000));
    let listener = TcpListener::bind(addr).await.unwrap();
    
    info!("此实例 ID: {}", core_id);
    info!("Xintong Rust Core 正在监听: {}", listener.local_addr().unwrap());
    
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

async fn handler_root() -> &'static str {
    "Xintong Rust Core (M1) 正在运行！"
}