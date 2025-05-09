// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

pub mod video_fixer;

use video_fixer::process_video;

#[tauri::command]
#[tokio::main]
async fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tokio::main]
async fn video() {
    let input_file = "path/to/your/video.mp4";
    process_video(input_file).await;
}
