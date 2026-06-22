"""
Download nemotron-3.5-asr-streaming-0.6b ONNX int4 weights to /opt/asr_model/
"""

from huggingface_hub import snapshot_download

REPO_ID = "onnx-community/nemotron-3.5-asr-streaming-0.6b-onnx-int4"
LOCAL_DIR = "/opt/asr_model"

print(f"Downloading {REPO_ID} → {LOCAL_DIR} ...")
snapshot_download(
    repo_id=REPO_ID,
    local_dir=LOCAL_DIR,
    local_dir_use_symlinks=False,
)
print(f"Done. Model saved to {LOCAL_DIR}")
