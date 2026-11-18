# Braid 品牌素材

[`logo/`](logo/) 是仓库内 Logo 的唯一素材源，完整保留 2026-10-19 交付包的目录和文件字节。包内的 [`README.md`](logo/README.md) 记录设计与制作说明，[`manifest.json`](logo/manifest.json) 以相对路径、字节数和 SHA-256 列出其余 130 个文件；校验清单本身不列入清单。

素材共有八款：彩色图形配透明、白色、黑色、深蓝圆角徽章背景，白色图形配透明或黑色背景，黑色图形配透明或白色背景。每款提供 16、20、24、32、48、64、128、256、512、1024 px PNG。

大尺寸使用 `logo/svg/standard/`。16–32 px 彩色图标使用 `logo/svg/small-optical-color/`；单色图标使用 `logo/svg/optical/<size>/` 中对应 16–128 px 的版本，保留小尺寸下的开口与编织轮廓。PNG 已按这些规则生成。SVG 不依赖字体、嵌入位图或外部资源；彩色版使用渐变、滤镜和裁剪，单色版使用路径和透明蒙版。

| 使用位置 | 直接使用的交付素材 |
| --- | --- |
| 根目录 `README.md`，显示为 96 × 96 px | `logo/png/braid-color-on-badge/braid-color-on-badge-128.png` |
| `src/setup/logo.rs`，合成 GitHub App 头像 | `logo/png/braid-color-on-transparent/braid-color-on-transparent-512.png` |
| Web Console 品牌图标 `/braid-logo.svg` | `logo/svg/small-optical-color/braid-color-on-transparent.svg` |
| Web Console favicon `/favicon.svg` | `logo/svg/small-optical-color/braid-color-on-badge.svg` |
| Web Console 备用 favicon `/favicon.png` | `logo/png/braid-color-on-badge/braid-color-on-badge-32.png` |

[`preview.png`](logo/preview.png) 展示全部款式，[`actual-size-preview.png`](logo/actual-size-preview.png) 展示单色小图的实际像素尺寸。`logo/source/` 保留原始高清参考图、已批准的单色轮廓和彩色版基线哈希；彩色 SVG 是冻结的制作输入。已有成品可直接使用，构建 Braid 无需重建素材。

如需重建，在 `logo/` 下依次运行 `python3 build_pack.py` 和 `python3 qa.py`。依赖 Python 3、Pillow、NumPy、SciPy 与 Inkscape CLI；预览生成还需要脚本指定的 `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf`，其他系统应先准备该字体路径或明确调整制作脚本。`qa.py` 会重新写入 `QA.json` 和 `manifest.json`，重建后须检查差异再接受新成品。
