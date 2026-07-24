use std::path::{Path, PathBuf};
use tokio::process::Command;

const WORKING_AUDIO_FILENAME: &str = "working.wav";

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MediaStorage {
    source_root: PathBuf,
    working_root: PathBuf,
    recordings_root: PathBuf,
    ffmpeg_bin: PathBuf,
    ffprobe_bin: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct StoredSourceMedia {
    pub path: PathBuf,
    pub filename: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRecordingAudio {
    pub path: PathBuf,
    pub filename: String,
}

impl MediaStorage {
    pub fn new(data_dir: impl Into<PathBuf>, ffmpeg_bin: PathBuf, ffprobe_bin: PathBuf) -> Self {
        let media_root = data_dir.into().join("media");
        Self {
            source_root: media_root.join("source"),
            working_root: media_root.join("working"),
            recordings_root: media_root.join("recordings"),
            ffmpeg_bin,
            ffprobe_bin,
        }
    }

    #[allow(dead_code)]
    pub async fn store_source_from_path(
        &self,
        source_path: &Path,
    ) -> Result<StoredSourceMedia, Box<dyn std::error::Error + Send + Sync>> {
        if !source_path.exists() {
            return Err(format!("Source media not found: {}", source_path.display()).into());
        }

        tokio::fs::create_dir_all(&self.source_root).await?;
        let filename = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(sanitize_filename)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "source-media".to_string());
        let unique_name = format!(
            "{}-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            filename
        );
        let dest = self.source_root.join(unique_name);
        tokio::fs::copy(source_path, &dest).await?;

        Ok(StoredSourceMedia {
            path: dest,
            filename,
        })
    }

    #[allow(dead_code)]
    pub async fn store_source_from_bytes(
        &self,
        filename: &str,
        bytes: &[u8],
    ) -> Result<StoredSourceMedia, Box<dyn std::error::Error + Send + Sync>> {
        if bytes.is_empty() {
            return Err("Uploaded media is empty".into());
        }

        tokio::fs::create_dir_all(&self.source_root).await?;
        let filename = sanitize_filename(filename);
        let filename = if filename.is_empty() {
            "source-media".to_string()
        } else {
            filename
        };
        let unique_name = format!(
            "{}-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            filename
        );
        let dest = self.source_root.join(unique_name);
        tokio::fs::write(&dest, bytes).await?;

        Ok(StoredSourceMedia {
            path: dest,
            filename,
        })
    }

    pub async fn prepare_standard_working_audio(
        &self,
        job_id: i64,
        source_path: &Path,
    ) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
        self.validate_audio_stream(source_path).await?;

        let job_working_dir = self.working_root.join(job_id.to_string());
        tokio::fs::create_dir_all(&job_working_dir).await?;
        let working_audio = job_working_dir.join(WORKING_AUDIO_FILENAME);
        let output = Command::new(&self.ffmpeg_bin)
            .arg("-y")
            .arg("-i")
            .arg(source_path)
            .arg("-vn")
            .arg("-ac")
            .arg("1")
            .arg("-ar")
            .arg("16000")
            .arg("-c:a")
            .arg("pcm_s16le")
            .arg(&working_audio)
            .output()
            .await
            .map_err(|error| format!("Failed to run ffmpeg: {error}"))?;

        if !output.status.success() {
            delete_file_and_parent_dir(&working_audio).await;
            return Err(format!(
                "ffmpeg failed while preparing working audio: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }

        Ok(working_audio)
    }

    pub async fn store_hotkey_recording(
        &self,
        recording_id: &str,
        source_wav_path: &Path,
    ) -> Result<StoredRecordingAudio, Box<dyn std::error::Error + Send + Sync>> {
        if !source_wav_path.exists() {
            return Err(format!("Recording not found: {}", source_wav_path.display()).into());
        }

        tokio::fs::create_dir_all(&self.recordings_root).await?;
        let safe_id = sanitize_filename(recording_id);
        let safe_id = if safe_id.is_empty() {
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
                .to_string()
        } else {
            safe_id
        };

        let mp3_filename = format!("kloyce-{safe_id}.mp3");
        let mp3_dest = self.recordings_root.join(&mp3_filename);
        match self
            .convert_recording_to_mp3(source_wav_path, &mp3_dest)
            .await
        {
            Ok(()) => {
                return Ok(StoredRecordingAudio {
                    path: mp3_dest,
                    filename: mp3_filename,
                });
            }
            Err(error) => {
                tracing::warn!(
                    source = %source_wav_path.display(),
                    dest = %mp3_dest.display(),
                    "Failed to convert hotkey recording to MP3, retaining WAV copy instead: {error}"
                );
                let _ = tokio::fs::remove_file(&mp3_dest).await;
            }
        }

        let wav_filename = format!("kloyce-{safe_id}.wav");
        let wav_dest = self.recordings_root.join(&wav_filename);
        tokio::fs::copy(source_wav_path, &wav_dest).await?;
        Ok(StoredRecordingAudio {
            path: wav_dest,
            filename: wav_filename,
        })
    }

    pub async fn delete_working_audio_path(&self, working_audio_path: &Path) {
        delete_file_and_parent_dir(working_audio_path).await;
    }

    pub async fn delete_retained_audio_path(
        &self,
        audio_path: &Path,
    ) -> Result<(), std::io::Error> {
        match tokio::fs::remove_file(audio_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub async fn delete_source_media_path(
        &self,
        source_media_path: &Path,
    ) -> Result<(), std::io::Error> {
        match tokio::fs::remove_file(source_media_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub async fn delete_all_working_audio(&self) {
        let _ = tokio::fs::remove_dir_all(&self.working_root).await;
    }

    async fn validate_audio_stream(
        &self,
        source_path: &Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let output = Command::new(&self.ffprobe_bin)
            .arg("-v")
            .arg("error")
            .arg("-select_streams")
            .arg("a:0")
            .arg("-show_entries")
            .arg("stream=codec_type")
            .arg("-of")
            .arg("default=noprint_wrappers=1:nokey=1")
            .arg(source_path)
            .output()
            .await
            .map_err(|error| format!("Failed to run ffprobe: {error}"))?;

        if !output.status.success() {
            return Err(format!(
                "ffprobe failed while validating media: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let has_audio_stream = stdout.lines().any(|line| line.trim() == "audio");
        if has_audio_stream {
            return Ok(());
        }

        Err("Source media has no audio stream".into())
    }

    async fn convert_recording_to_mp3(
        &self,
        source_wav_path: &Path,
        mp3_dest: &Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let output = Command::new(&self.ffmpeg_bin)
            .arg("-y")
            .arg("-i")
            .arg(source_wav_path)
            .arg("-vn")
            .arg("-codec:a")
            .arg("libmp3lame")
            .arg("-b:a")
            .arg("128k")
            .arg(mp3_dest)
            .output()
            .await
            .map_err(|error| format!("Failed to run ffmpeg: {error}"))?;

        if !output.status.success() {
            return Err(format!(
                "ffmpeg failed while retaining recording as MP3: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }

        Ok(())
    }
}

async fn delete_file_and_parent_dir(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
}

#[allow(dead_code)]
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kloyce-media-test-{}-{}-{name}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn fake_tool(dir: &Path, name: &str, script: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[tokio::test]
    async fn rejects_media_without_audio_stream_before_ffmpeg() {
        let dir = temp_dir("no-audio");
        let source = dir.join("video.mp4");
        std::fs::write(&source, b"fake video").unwrap();
        let ffprobe = fake_tool(&dir, "ffprobe", "#!/bin/sh\nexit 0\n");
        let ffmpeg = fake_tool(&dir, "ffmpeg", "#!/bin/sh\nexit 42\n");
        let storage = MediaStorage::new(dir.clone(), ffmpeg, ffprobe);

        let error = storage
            .prepare_standard_working_audio(42, &source)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("no audio stream"));
        assert!(!dir.join("media/working/42/working.wav").exists());
    }

    #[tokio::test]
    async fn prepares_predictable_standard_working_wav() {
        let dir = temp_dir("working-wav");
        let source = dir.join("clip.mp3");
        std::fs::write(&source, b"fake mp3").unwrap();
        let ffprobe = fake_tool(&dir, "ffprobe", "#!/bin/sh\nprintf 'audio\\n'\n");
        let ffmpeg = fake_tool(
            &dir,
            "ffmpeg",
            "#!/bin/sh\nfor out; do :; done\nprintf 'wav' > \"$out\"\n",
        );
        let storage = MediaStorage::new(dir.clone(), ffmpeg, ffprobe);

        let working_audio = storage
            .prepare_standard_working_audio(7, &source)
            .await
            .unwrap();

        assert_eq!(working_audio, dir.join("media/working/7/working.wav"));
        assert_eq!(std::fs::read_to_string(&working_audio).unwrap(), "wav");
    }

    #[tokio::test]
    async fn stores_source_media_under_daemon_owned_storage() {
        let dir = temp_dir("source-storage");
        let source = dir.join("Unsafe Name.mp4");
        std::fs::write(&source, b"source").unwrap();
        let storage = MediaStorage::new(
            dir.clone(),
            PathBuf::from("ffmpeg"),
            PathBuf::from("ffprobe"),
        );

        let stored = storage.store_source_from_path(&source).await.unwrap();

        assert_eq!(stored.filename, "Unsafe_Name.mp4");
        assert!(stored.path.starts_with(dir.join("media/source")));
        assert_eq!(std::fs::read(&stored.path).unwrap(), b"source");
    }

    #[tokio::test]
    async fn stores_uploaded_source_media_bytes_under_daemon_owned_storage() {
        let dir = temp_dir("upload-source-storage");
        let storage = MediaStorage::new(
            dir.clone(),
            PathBuf::from("ffmpeg"),
            PathBuf::from("ffprobe"),
        );

        let stored = storage
            .store_source_from_bytes("Unsafe Upload.mp3", b"uploaded media")
            .await
            .unwrap();

        assert_eq!(stored.filename, "Unsafe_Upload.mp3");
        assert!(stored.path.starts_with(dir.join("media/source")));
        assert_eq!(std::fs::read(&stored.path).unwrap(), b"uploaded media");
    }

    #[tokio::test]
    async fn deletes_source_media_file_without_removing_source_root() {
        let dir = temp_dir("delete-source-storage");
        let storage = MediaStorage::new(
            dir.clone(),
            PathBuf::from("ffmpeg"),
            PathBuf::from("ffprobe"),
        );
        let stored = storage
            .store_source_from_bytes("clip.wav", b"source")
            .await
            .unwrap();

        storage
            .delete_source_media_path(&stored.path)
            .await
            .unwrap();

        assert!(!stored.path.exists());
        assert!(dir.join("media/source").exists());
    }

    #[tokio::test]
    async fn stores_hotkey_recording_as_mp3_when_ffmpeg_succeeds() {
        let dir = temp_dir("hotkey-mp3");
        let source = dir.join("recording.wav");
        std::fs::write(&source, b"wav").unwrap();
        let ffmpeg = fake_tool(
            &dir,
            "ffmpeg",
            "#!/bin/sh\nfor out; do :; done\nprintf 'mp3' > \"$out\"\n",
        );
        let storage = MediaStorage::new(dir.clone(), ffmpeg, PathBuf::from("ffprobe"));

        let stored = storage
            .store_hotkey_recording("20260630-120000.000000", &source)
            .await
            .unwrap();

        assert_eq!(stored.filename, "kloyce-20260630-120000.000000.mp3");
        assert_eq!(
            stored.path,
            dir.join("media/recordings").join(&stored.filename)
        );
        assert_eq!(std::fs::read_to_string(&stored.path).unwrap(), "mp3");
    }

    #[tokio::test]
    async fn stores_hotkey_recording_as_wav_when_mp3_conversion_fails() {
        let dir = temp_dir("hotkey-wav-fallback");
        let source = dir.join("recording.wav");
        std::fs::write(&source, b"wav").unwrap();
        let ffmpeg = fake_tool(&dir, "ffmpeg", "#!/bin/sh\nexit 42\n");
        let storage = MediaStorage::new(dir.clone(), ffmpeg, PathBuf::from("ffprobe"));

        let stored = storage
            .store_hotkey_recording("unsafe recording id", &source)
            .await
            .unwrap();

        assert_eq!(stored.filename, "kloyce-unsafe_recording_id.wav");
        assert_eq!(
            stored.path,
            dir.join("media/recordings").join(&stored.filename)
        );
        assert_eq!(std::fs::read(&stored.path).unwrap(), b"wav");

        storage
            .delete_retained_audio_path(&stored.path)
            .await
            .unwrap();
        assert!(!stored.path.exists());
        assert!(dir.join("media/recordings").exists());
    }
}
