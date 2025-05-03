use once_cell::sync::Lazy;
use std::env;
use std::fs;
use std::fs::File;
use std::io::Cursor;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use tempfile::tempdir;
use tokio::sync::Semaphore;

const FFMPEG_EXECUTABLE: &[u8] = if cfg!(target_os = "windows") {
    include_bytes!("resources/ffmpeg-windows.zst")
} else if cfg!(target_os = "macos") {
    include_bytes!("resources/ffmpeg-mac.zst")
} else if cfg!(target_os = "linux") {
    include_bytes!("resources/ffmpeg-linux.zst")
} else {
    include_bytes!("resources/ffmpeg-linux.zst")
};

static FFMPEG_PATH: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

fn extract_ffmpeg() -> std::io::Result<String> {
    use zstd::stream::read::Decoder;
    let temp_dir = env::temp_dir();
    let ffmpeg_path = temp_dir.join("ffmpeg");

    let compressed = Cursor::new(FFMPEG_EXECUTABLE);
    let mut decoder = Decoder::new(compressed)?;
    let mut out = File::create(&ffmpeg_path)?;
    std::io::copy(&mut decoder, &mut out)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = out.metadata()?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&ffmpeg_path, perms)?;
    }

    Ok(ffmpeg_path.to_string_lossy().into_owned())
}

fn get_ffmpeg_path() -> String {
    let mut cached = FFMPEG_PATH.lock().unwrap();
    if cached.is_none() {
        match extract_ffmpeg() {
            Ok(path) => *cached = Some(path),
            Err(e) => {
                eprintln!("Failed to extract ffmpeg!  :{}", e);
                std::process::exit(1);
            }
        }
    }
    cached.clone().unwrap()
}

fn collect_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "png" {
                        files.push(path);
                    }
                }
            } else if path.is_dir() {
                files.extend(collect_files(&path));
            }
        }
    }

    files
}

fn stitch_frames_into_video(folder: &str, output_file: &str) {
    let ffmpeg_path = get_ffmpeg_path();

    let input_pattern = format!("{}/frame_%04d.png", folder);

    let status = Command::new(ffmpeg_path)
        .args(&[
            "-framerate",
            "30",
            "-i",
            &input_pattern,
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            output_file,
        ])
        .status()
        .expect("Failed to stitch frames into video");

    if !status.success() {
        eprintln!("FFmpeg failed to stitch video");
    }
}

fn generate_frames(input_file: &str) -> (String, tempfile::TempDir) {
    let temp_dir = tempdir().expect("Failed to create temp directory");
    let output_pattern = temp_dir.path().join("frame_%04d.png");
    let ffmpeg_path = get_ffmpeg_path();

    let output_pattern_str = output_pattern.to_str().unwrap();

    Command::new(ffmpeg_path)
        .args(&["-i", input_file, output_pattern_str])
        .output()
        .expect("Failed to execute ffmpeg");

    (
        output_pattern
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
        temp_dir,
    )
}

fn compare_images_ssim(image1: &str, image2: &str) -> f32 {
    let output = Command::new(get_ffmpeg_path())
        .arg("-i")
        .arg(image1)
        .arg("-i")
        .arg(image2)
        .arg("-filter_complex")
        .arg("ssim")
        .arg("-f")
        .arg("null")
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to execute FFmpeg");

    let result = String::from_utf8_lossy(&output.stderr);

    // Parse the SSIM score from FFmpeg output
    // SSIM output looks like: "SSIM: All: 0.978"
    if let Some(ssim_value) = result.split("All: ").nth(1) {
        let ssim_score: f32 = ssim_value
            .split_whitespace()
            .next()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0.0);
        return ssim_score;
    }

    0.0
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <video_file>", args[0]);
        std::process::exit(1);
    }

    let input_file = &args[1];

    let (frames_folder, _temp_dir) = generate_frames(input_file);

    let frames_vec: Vec<PathBuf> = collect_files(Path::new(&frames_folder));

    let semaphore = Arc::new(Semaphore::new(4));

    let mut handles = Vec::new();

    for index in 0..frames_vec.len() - 1 {
        let image1 = frames_vec[index].clone();
        let image2 = frames_vec[index + 1].clone();
        let permit = semaphore.clone().acquire_owned().await.unwrap();

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let score = compare_images_ssim(&image1.to_string_lossy(), &image2.to_string_lossy());
            score > 0.95
        });

        handles.push(handle);
    }

    let mut bad_frames: Vec<bool> = Vec::new();

    for handle in handles {
        match handle.await {
            Ok(result) => bad_frames.push(result),
            Err(_) => bad_frames.push(false),
        }
    }
    bad_frames.push(false);

    for (index, value) in frames_vec.iter().enumerate() {
        if bad_frames[index] {
            match fs::remove_file(value) {
                Ok(_) => println!("removed dead frame"),
                Err(e) => eprintln!("failed to delete file {}", e),
            }
        }
    }

    let output_video = format!(
        "{}_processed.mp4",
        Path::new(input_file).file_stem().unwrap().to_str().unwrap()
    );
    stitch_frames_into_video(&frames_folder, &output_video);
    println!("Video created: {}", output_video);
}
