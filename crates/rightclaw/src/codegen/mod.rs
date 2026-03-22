pub mod process_compose;
pub mod shell_wrapper;
pub mod system_prompt;

pub use process_compose::generate_process_compose;
pub use shell_wrapper::generate_wrapper;
pub use system_prompt::generate_combined_prompt;
