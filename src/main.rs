#[macro_use]
extern crate cursive;
#[macro_use]
extern crate serde;

use std::{path::PathBuf, process::exit};

use application::{setup_logging, Application};
use config::set_configuration_base_path;
use log::error;
use ncspot::program_arguments;

mod application;
mod authentication;
mod cli;
mod command;
mod commands;
mod config;
mod events;
mod ext_traits;
mod library;
mod model;
mod panic;
mod queue;
mod serialization;
mod sharing;
mod spotify;
mod spotify_api;
mod spotify_url;
mod spotify_worker;
mod theme;
mod traits;
mod ui;
mod utils;

#[cfg(unix)]
mod ipc;

#[cfg(feature = "mpris")]
mod mpris;

fn main() -> Result<(), String> {
    // Set a custom backtrace hook that writes the backtrace to a file instead of stdout, since
    // stdout is most likely in use by Cursive.
    // 类似于全局panic捕获？输出标准流到backtrace.log文件中
    panic::register_backtrace_panic_handler();

    // Parse the command line arguments.
    // 解析命令行参数
    let matches = program_arguments().get_matches();

    // Enable debug logging to a file if specified on the command line.
    // 启用日志，并配置日志输出文件，使用fern库记录日志
    // 命令中获取debug的参数配置
    if let Some(filename) = matches.get_one::<PathBuf>("debug") {
        setup_logging(filename).expect("logger could not be initialized");
    }

    // Set the configuration base path. All configuration files are read/written relative to this
    // path.
    // 配置基路径
    set_configuration_base_path(matches.get_one::<PathBuf>("basepath").cloned());

    match matches.subcommand() {
        // 打印info信息
        Some(("info", _subcommand_matches)) => cli::info(),
        // 类似于panic!
        Some((_, _)) => unreachable!(),
        None => {
            // Create the application.
            // 构建应用
            let mut application =
                match Application::new(matches.get_one::<String>("config").cloned()) {
                    Ok(application) => application,
                    Err(error) => {
                        eprintln!("{error}");
                        error!("{error}");
                        exit(-1);
                    }
                };

            // Start the application event loop.
            // 运行应用
            application.run()
        }
    }?;

    Ok(())
}
