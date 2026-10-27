"""Trains the building detector and exports it to ONNX for the Rust bot."""

import shutil
from pathlib import Path

from ultralytics import YOLO

ROOT = Path(__file__).resolve().parent.parent
DATASET = ROOT / "dataset"
CLASSES = (ROOT / "training" / "classes.txt").read_text().split()


def write_dataset_yaml() -> Path:
    path = DATASET / "data.yaml"
    names = "\n".join(f"  {i}: {name}" for i, name in enumerate(CLASSES))
    path.write_text(
        f"path: {DATASET.as_posix()}\ntrain: images/train\nval: images/val\nnames:\n{names}\n"
    )
    return path


def main() -> None:
    data = write_dataset_yaml()
    # The frames are 1280x720; train at full width so small buildings survive.
    model = YOLO("yolo11n.pt")
    model.train(data=str(data), imgsz=1280, epochs=150, batch=8, device=0,
                project=str(ROOT / "training" / "runs"), name="buildings", exist_ok=True)
    onnx = Path(model.export(format="onnx", imgsz=1280, dynamic=False))
    (ROOT / "models").mkdir(exist_ok=True)
    shutil.copy(onnx, ROOT / "models" / "buildings.onnx")
    print(f"exported to {ROOT / 'models' / 'buildings.onnx'}")


if __name__ == "__main__":
    main()
