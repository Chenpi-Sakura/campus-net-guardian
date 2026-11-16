//! 校园网守护核心：配置、认证、平台适配与串行后台状态机。
pub mod auth;
pub mod config;
pub mod daemon;
pub mod platform;
pub mod running;
pub mod running_platform;
#[cfg(windows)]
pub mod ui;
