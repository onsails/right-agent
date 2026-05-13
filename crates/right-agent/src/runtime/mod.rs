pub mod deps;
pub mod pc_client;

pub use deps::verify_dependencies;
pub use pc_client::{PcClient, ProcessInfo};
