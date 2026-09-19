#![allow(dead_code)]

use origin_speak_lib::{
    TranscriptionComputeBackend, TranscriptionRuntimePhase, TranscriptionRuntimeStatus,
    app_local_data_root,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const STATUS_FILE: &str = "runtime-compute-status.json";
const STATUS_TEMP_FILE: &str = "runtime-compute-status.json.tmp";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RuntimeComputeRecord {
    pub model: String,
    pub phase: String,
    pub gpu_requested: bool,
    pub compute: Option<String>,
    pub accelerator: Option<String>,
    pub fallback_reason: Option<String>,
}

impl RuntimeComputeRecord {
    fn from_status(status: &TranscriptionRuntimeStatus) -> Self {
        Self {
            model: status.model.clone(),
            phase: phase_label(&status.phase).to_string(),
            gpu_requested: status.gpu_requested,
            compute: status.active_backend.map(|backend| match backend {
                TranscriptionComputeBackend::Cpu => "CPU".to_string(),
                TranscriptionComputeBackend::Gpu => "GPU".to_string(),
            }),
            accelerator: status.accelerator.clone(),
            fallback_reason: status.backend_fallback_reason.clone(),
        }
    }
}

pub fn publish(status: &TranscriptionRuntimeStatus) -> Result<(), String> {
    let root = app_local_data_root()?;
    fs::create_dir_all(&root).map_err(|error| {
        format!(
            "create runtime status directory {}: {error}",
            root.display()
        )
    })?;
    let target = root.join(STATUS_FILE);
    let temporary = root.join(STATUS_TEMP_FILE);
    let payload = serde_json::to_vec_pretty(&RuntimeComputeRecord::from_status(status))
        .map_err(|error| format!("serialize runtime compute status: {error}"))?;
    fs::write(&temporary, payload).map_err(|error| {
        format!(
            "write temporary runtime compute status {}: {error}",
            temporary.display()
        )
    })?;
    if target.exists() {
        fs::remove_file(&target).map_err(|error| {
            format!(
                "replace runtime compute status {}: {error}",
                target.display()
            )
        })?;
    }
    fs::rename(&temporary, &target).map_err(|error| {
        format!(
            "publish runtime compute status {}: {error}",
            target.display()
        )
    })
}

pub fn read() -> Result<Option<RuntimeComputeRecord>, String> {
    let path = status_path()?;
    if !path.is_file() {
        return Ok(None);
    }
    let payload = fs::read(&path)
        .map_err(|error| format!("read runtime compute status {}: {error}", path.display()))?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|error| format!("parse runtime compute status {}: {error}", path.display()))
}

pub fn clear() -> Result<(), String> {
    for path in [status_path()?, temp_status_path()?] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "remove runtime compute status {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn status_path() -> Result<PathBuf, String> {
    Ok(app_local_data_root()?.join(STATUS_FILE))
}

fn temp_status_path() -> Result<PathBuf, String> {
    Ok(app_local_data_root()?.join(STATUS_TEMP_FILE))
}

fn phase_label(phase: &TranscriptionRuntimePhase) -> &'static str {
    match phase {
        TranscriptionRuntimePhase::Ready => "ready",
        TranscriptionRuntimePhase::ModelMissing => "model-missing",
        TranscriptionRuntimePhase::Downloading => "downloading",
        TranscriptionRuntimePhase::Verifying => "verifying",
        TranscriptionRuntimePhase::Loading => "loading",
        TranscriptionRuntimePhase::Transcribing => "transcribing",
        TranscriptionRuntimePhase::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_status_reports_actual_compute_not_only_preference() {
        let status = TranscriptionRuntimeStatus {
            phase: TranscriptionRuntimePhase::Ready,
            model: "canary-qwen-2.5b".to_string(),
            model_path: "model.gguf".to_string(),
            model_downloaded: true,
            download_bytes: None,
            download_total_bytes: None,
            gpu_requested: true,
            active_backend: Some(TranscriptionComputeBackend::Gpu),
            accelerator: Some("NVIDIA GeForce RTX 4080".to_string()),
            backend_fallback_reason: None,
            last_error: None,
        };
        let record = RuntimeComputeRecord::from_status(&status);
        assert_eq!(record.compute.as_deref(), Some("GPU"));
        assert_eq!(
            record.accelerator.as_deref(),
            Some("NVIDIA GeForce RTX 4080")
        );
    }
}
