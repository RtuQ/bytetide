//! 串口/网络数据源：配置、会话管理（ring 拉模型）、规则评估。
//! 热插拔监听属 GUI 关注点，留在桌面端 crate。

pub mod manager;
pub mod port;
pub mod rules;

pub use manager::PortManager;
