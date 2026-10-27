# Building detection training

Python is used only here, to train the model. The bot runs the exported ONNX
model from Rust.

## 1. Collect

With `save_bases = true` in `config.toml`, every base the bot scouts is saved
to `dataset/raw/`. A few hundred bases covering your Town Hall range is a good
start.

## 2. Label

Label boxes around buildings with any tool that exports **YOLO format**. Both
of these run locally:

- [X-AnyLabeling](https://github.com/CVHub520/X-AnyLabeling) (desktop app)
- [Label Studio](https://labelstud.io/) (`pip install label-studio`, runs in the browser on localhost)

Use the classes in [classes.txt](classes.txt), in that order. Put the result in:

```
dataset/
  images/train/*.png   labels/train/*.txt
  images/val/*.png     labels/val/*.txt    (about 15% of the images)
```

## 3. Train and export

Needs Python 3.10+ and an NVIDIA GPU (CUDA).

```sh
pip install ultralytics
python training/train.py
```

This trains a small YOLO model and writes `models/buildings.onnx`. The bot
loads it from there.
