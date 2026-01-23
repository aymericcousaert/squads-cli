use colored::Colorize;
use serde::Serialize;
use tabled::{Table, Tabled};

use super::OutputFormat;

/// Print data in the specified format
pub fn print_output<T: Serialize + Tabled>(data: &[T], format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(data).unwrap();
            println!("{}", json);
        }
        OutputFormat::Table => {
            let table = Table::new(data).to_string();
            println!("{}", table);
        }
        OutputFormat::Plain => {
            // For plain format, serialize to JSON and extract values
            let json = serde_json::to_value(data).unwrap();
            if let Some(arr) = json.as_array() {
                for item in arr {
                    if let Some(obj) = item.as_object() {
                        let values: Vec<String> = obj
                            .values()
                            .map(|v| match v {
                                serde_json::Value::String(s) => s.clone(),
                                serde_json::Value::Null => "".to_string(),
                                other => other.to_string(),
                            })
                            .collect();
                        println!("{}", values.join("|"));
                    }
                }
            }
        }
    }
}

/// Print a single item in the specified format
pub fn print_single<T: Serialize>(data: &T, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(data).unwrap();
            println!("{}", json);
        }
        OutputFormat::Table | OutputFormat::Plain => {
            let json = serde_json::to_string_pretty(data).unwrap();
            println!("{}", json);
        }
    }
}

/// Print success message
pub fn print_success(message: &str) {
    println!("{} {}", "✓".green(), message);
}

/// Print error message
pub fn print_error(message: &str) {
    eprintln!("{} {}", "✗".red(), message);
}

/// Print info message
pub fn print_info(message: &str) {
    println!("{} {}", "ℹ".blue(), message);
}

/// Print warning message
pub fn print_warning(message: &str) {
    println!("{} {}", "⚠".yellow(), message);
}
