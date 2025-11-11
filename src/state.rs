// 用于管理我们的 AppState
// src/state.rs（状态管理器）
//这个文件定义了“7日版”Python Core 中的 ConnectionManager 和 RedisRegistry 的 Rust 等价物。

// src/state.rs (修复版)

// src/state.rs (修复版)

use axum::extract::ws::{Message, WebSocket};
use deadpool_redis::{
    redis::{self, AsyncCommands},
    Pool,
};
use futures_util::stream::SplitSink;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

// 1. 定义本地连接的类型别名
pub type LocalConnectionMap = Arc<Mutex<HashMap<String, SplitSink<WebSocket, Message>>>>;

// 2. 定义我们的 AppState
#[derive(Clone)]
pub struct AppState {
    pub redis_pool: Pool,
    // pub redis_client: redis::Client, // <-- 修复: 移除未使用的字段
    pub core_instance_id: String,
    pub local_connections: LocalConnectionMap,
}

// 3. 实现 RedisRegistry 逻辑
impl AppState {
    /// 在 Redis 中注册 Agent 的位置和能力
    pub async fn register_agent(&self, agent_id: &str, capabilities: &[String]) {
        let key_loc = format!("agent:location:{}", agent_id);
        let key_caps = format!("agent:caps:{}", agent_id);
        let caps_json = serde_json::to_string(capabilities).unwrap_or_else(|_| "[]".to_string());

        let mut conn = match self.redis_pool.get().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("[Redis] 注册时无法获取连接: {}", e);
                return;
            }
        };

        let pipe_result: redis::RedisResult<((), ())> = redis::pipe()
            .set_ex(&key_loc, &self.core_instance_id, 60)
            .set_ex(&key_caps, &caps_json, 60)
            .query_async(&mut *conn)
            .await;

        if pipe_result.is_ok() {
            info!(
                "[Redis] 已注册 Agent '{}' 在 {}",
                agent_id,
                &self.core_instance_id[..8]
            );
        } else {
            error!("[Redis] 注册 Agent '{}' 失败: {:?}", agent_id, pipe_result.err());
        }
    }

    /// 从 Redis 中注销 Agent
    pub async fn unregister_agent(&self, agent_id: &str) {
        let key_loc = format!("agent:location:{}", agent_id);
        let key_caps = format!("agent:caps:{}", agent_id);

        let mut conn = match self.redis_pool.get().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("[Redis] 注销时无法获取连接: {}", e);
                return;
            }
        };

        let _: redis::RedisResult<()> = redis::pipe()
            .del(&key_loc)
            .del(&key_caps)
            .query_async(&mut *conn)
            .await;

        info!("[Redis] 已注销 Agent '{}'", agent_id);
    }

    /// 查找 Agent 在哪个 Core 实例上
    pub async fn find_agent_location(&self, agent_id: &str) -> Option<String> {
        let key_loc = format!("agent:location:{}", agent_id);
        
        let mut conn = match self.redis_pool.get().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("[Redis] 查找时无法获取连接: {}", e);
                return None;
            }
        };

        match conn.get(key_loc).await {
            Ok(location) => Some(location),
            Err(_) => None,
        }
    }

    /// 刷新 Agent 键的过期时间 (心跳)
    pub async fn keep_alive(&self, agent_id: &str) {
        let key_loc = format!("agent:location:{}", agent_id);
        let key_caps = format!("agent:caps:{}", agent_id);

        let mut conn = match self.redis_pool.get().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("[Redis] 心跳时无法获取连接: {}", e);
                return;
            }
        };

        let pipe_result: redis::RedisResult<((), ())> = redis::pipe()
            .expire(&key_loc, 60)
            .expire(&key_caps, 60)
            .query_async(&mut *conn)
            .await;

        if pipe_result.is_ok() {
            info!("[{}] 刷新了心跳。", agent_id);
        } else {
            warn!("[{}] 心跳失败 (可能 Agent 已过期): {:?}", agent_id, pipe_result.err());
        }
    }

    /// 将消息发布到 Redis Pub/Sub
    pub async fn publish_message(&self, channel: &str, message: &str) {
        let mut conn = match self.redis_pool.get().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("[PubSub] 发布时无法获取连接: {}", e);
                return;
            }
        };
        let _: redis::RedisResult<()> = conn.publish(channel, message).await;
    }
}