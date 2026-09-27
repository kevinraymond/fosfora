#[cfg(feature = "desktop")]
pub mod hot_reload;

#[cfg(feature = "desktop")]
pub use hot_reload::ShaderWatcher;
