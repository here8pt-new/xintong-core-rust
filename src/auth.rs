// src/auth.rs
// M2 任务 3: JWT 认证模块

use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::warn;

// 1. 定义我们 Token 的“声明” (Claims)
// 我们将允许 Agent 在 Token 中声明自己的 ID
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenClaims {
    pub sub: String, // "subject" (即 agent_id)
    pub exp: usize,  // "expiration" (过期时间)
    pub iat: usize,  // "issued at" (签发时间)
}

// 2. 密钥管理 (简单版)
// (在生产中，这应该来自一个安全的配置文件或环境变量)
const JWT_SECRET: &str = "my_super_secret_and_long_key_for_xintong_m2";

// 延迟加载的密钥，以避免重复计算
lazy_static::lazy_static! {
    static ref ENCODING_KEY: EncodingKey = EncodingKey::from_secret(JWT_SECRET.as_bytes());
    static ref DECODING_KEY: DecodingKey = DecodingKey::from_secret(JWT_SECRET.as_bytes());
}

/// 验证一个传入的 Token 字符串
pub fn validate_token(token: &str, expected_agent_id: &str) -> Result<TokenClaims, String> {
    let validation = Validation::default();
    
    match decode::<TokenClaims>(token, &DECODING_KEY, &validation) {
        Ok(token_data) => {
            // 检查 Token 中的 'sub' (agent_id) 是否与
            // URL 路径中的 'agent_id' 匹配
            if token_data.claims.sub != expected_agent_id {
                warn!("[Auth] Token 'sub' ({}) 与路径 ID ({}) 不匹配", token_data.claims.sub, expected_agent_id);
                Err("Token subject does not match Agent ID".to_string())
            } else {
                Ok(token_data.claims)
            }
        }
        Err(e) => {
            warn!("[Auth] Token 验证失败: {}", e);
            Err(e.to_string())
        }
    }
}

/// (工具函数) 为 Agent 生成一个测试 Token
/// (我们稍后可以用它来为我们的 Python/JS Agent 生成令牌)
#[allow(dead_code)] // 告诉编译器：我们知道这个没被使用
pub fn generate_test_token(agent_id: &str) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let claims = TokenClaims {
        sub: agent_id.to_string(),
        iat: now as usize,
        exp: (now + Duration::from_secs(60 * 60 * 24 * 7).as_secs()) as usize, // 7 天后过期
    };

    encode(&Header::default(), &claims, &ENCODING_KEY).unwrap()
}