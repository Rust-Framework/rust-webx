//! Device context — 设备实例 CRUD + 批量生成.

mod create_device;
mod delete_device;
mod generate_devices;
mod get_device;
mod list_devices;
mod list_spec_devices;
pub mod queries;
mod update_device;

pub use create_device::CreateDeviceHandler;
pub use delete_device::DeleteDeviceHandler;
pub use generate_devices::GenerateDevicesHandler;
pub use get_device::GetDeviceHandler;
pub use list_devices::ListDevicesHandler;
pub use list_spec_devices::ListSpecDevicesHandler;
pub use update_device::UpdateDeviceHandler;
