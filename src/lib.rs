#[macro_export]
macro_rules! debug_log {
	($($arg:tt)*) => {{
		#[cfg(debug_assertions)]
		{
			eprintln!($($arg)*);
		}
	}};
}

pub mod config;
pub mod scanner;
pub mod mapper;
pub mod slicer;
pub mod xml_builder;
pub mod inspector;
pub mod universal;
pub mod vector_store;
pub mod server;
pub mod workspace;
