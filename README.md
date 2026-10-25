# pngdeer

有损 PNG 压缩：每张图找到达到目标画质（SSIMULACRA2 分数）的最小调色板 PNG。画质默认按图自动决定，和 TinyPNG 一样不用选档位。命令行和浏览器版是同一份代码，同一张图输出逐字节相同。MIT 或 Apache-2.0 许可。

## 效果

两组图上和 TinyPNG 比（`bench/bench.py`，TinyPNG 用它的 API）。109 张用来调参数，71 张留出、调参时没看过；都是图标、界面截图、插画、照片、带透明的抠图和 3D 渲染：

| 对 TinyPNG | 109 张 | 71 张（留出） |
|---|---|---|
| 同 SSIMULACRA2 画质下的体积（中位数 / 总） | 84.6% / 81.6% | 83.3% / 79.0% |
| 同 butteraugli 画质下的体积（中位数 / 总） | 87.2% / 83.0% | 86.0% / 79.5% |
| TinyPNG 的画质够不着 | 6 张 | 4 张 |
| 自动画质（默认）：总体积 / 分数比 TinyPNG 高（中位数） | 89.0% / +0.7 | 84.2% / +0.6 |

“同画质下的体积”是把各个目标分数的结果连成体积–画质曲线，看达到 TinyPNG 那张图的分数要多少字节。butteraugli 是另一个独立的画质指标，搜索时没用它。上面是默认设置（大图先在缩小图上搜索）；`--preview-pixels 0` 在原图上完整搜索，还能再小约 1 个百分点，但慢几倍。图集是我们自己挑的、只有 180 张，结论的适用范围以此为限。怎么一步步做到的，见 [FINDINGS.md](experiments/palette-feedback/FINDINGS.md)。

## 命令行

```bash
cargo build --release
```

```bash
target/release/pngdeer in.png -o out.png
```

- `--target 85`：改成达到固定的 SSIMULACRA2 分数（100 = 一致，90 = 1:1 看不出区别，80 = 高画质）；默认 `auto`，按图决定
- `--preview-pixels N`：超过 N 像素的图先在缩小图上搜索，再用原图确认，默认 350000（和浏览器版一样）；`0` 在原图上完整搜索
- `--variants 50,0,0r2,w0`：每个颜色数上尝试的重新映射方式（抖动强度，`r2` 再做邻居平滑，`w` 开头用 Wu 的调色板）

输出写到 `out.png`，标准输出打印一行 JSON：目标分数、颜色数、选中的变体、分数、评估次数、各阶段耗时、结果类型（`hit`、`miss` 够不着时取最接近的、`exact` 少色图无损、`lossless` 调色板比原图还大时无损重压、`original` 原样保留）和字节数。

## 浏览器版

`web/` 是一个静态页面，也是项目主页：拖进 PNG，在本机用 WebAssembly 压缩，图片不上传；压完可以拖动分割线对比原图和结果，下面是和 TinyPNG 及其他开源工具的对比（数据来自下文的基准测试）。`web/brand/` 里的 HeyDeer 标志取自 [heydeer.ai/brand](https://heydeer.ai/brand)，按那里的规范使用；字体从 Google Fonts 加载。`build.sh` 编两份：`web/pkg` 单线程，哪里都能跑；`web/pkg-mt` 多线程（wasm-bindgen-rayon），只在跨源隔离的页面里能用（响应头 `Cross-Origin-Opener-Policy: same-origin` 和 `Cross-Origin-Embedder-Policy: require-corp`），页面自己判断用哪份。

构建需要 rustup 的 stable 和 nightly 工具链（都带 wasm32 目标，nightly 还要 rust-src，多线程版要用它重新编译标准库）、与 `web/wasm/Cargo.toml` 同版本的 wasm-bindgen-cli，以及能编译到 wasm32 的 clang（oxipng 用的 libdeflate 是 C 写的；苹果自带的 clang 不行，Homebrew 的 llvm 可以，`build.sh` 会自动找，也可以用 `LLVM_BIN` 指定）。用 `--no-modify-path` 安装 rustup 不会改动 PATH，不影响系统里已有的 cargo：

```bash
brew install llvm
```

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --target wasm32-unknown-unknown
```

```bash
~/.cargo/bin/rustup toolchain install nightly --profile minimal --component rust-src --target wasm32-unknown-unknown
```

```bash
~/.cargo/bin/cargo install wasm-bindgen-cli --version 0.2.129 --locked
```

```bash
sh web/build.sh
```

```bash
python3 web/serve.py 8770
```

然后打开 <http://localhost:8770/>。`serve.py` 就是带上那两个响应头的静态服务器；部署到别处也要能设这两个头，否则页面退回单线程（几个 worker 各压一张）。Vercel 上的部署配置在 `vercel.json`：构建时 `web/vercel-build.sh` 装好上面的工具链、编两份 WASM，再把页面复制到 `dist/`（示例图的符号链接换成真文件），响应头也在 `vercel.json` 里加。多线程版每次一张图，用上所有核；它先压一张 16×16 的小图自检，15 秒内（含下载和启动各线程）没完成就退回单线程。超过 1200 万像素的图交给单线程版（多线程版内存用尽会卡住，单线程版会正常报错），超过 1600 万像素直接拒绝（wasm32 最多 4 GB）。5MP 截图在 Chrome 里多线程每张 3.5–8 秒，单线程 8–20 秒。

页面链接的 `web/THIRD_PARTY_LICENSES.txt` 列出编进 WASM 的所有第三方代码及其许可，依赖变了以后用 `python3 web/notices.py` 重新生成。

## 怎么做的

- **搜索**（`src/search.rs`）：对每种重新映射方式，在 2–256 个颜色之间按对数二分，找到刚好达到目标分数的最少颜色，几种方式里取文件最小的。候选在内存里用 [fast-ssim2](https://github.com/imazen/fast-ssim2) 打分，原图的参考数据只算一次。大图先在缩小图上搜索，再用原图打分确认（最多 3 次）。256 色都够不着时，再试满抖动、普通调色板和精修调色板，多出的字节划算才用。
- **自动画质**：TinyPNG 的画质和“普通 256 色调色板能给的画质”走得很近，所以目标就取这个分数（减一个校准过的偏移），最高 90。
- **调色板**（`src/palette.rs`）：加权中位切分加 k-means；先量化一遍，按每个像素的误差加权再量化一遍，让少而醒目的颜色（强调色、文字）也能分到位置。不透明的图另外试 Wu 量化器加在线 k-means 的调色板，照片常靠它更小。
- **颜色**（`src/color.rs`）：在 Oklab 里算距离；带透明的颜色按叠在深灰和浅灰两种背景上的样子比较，和打分时一致。
- **映射**（`src/remap.rs`）：最近色，或蛇形 Floyd–Steinberg，在边缘、纹理和半透明像素处不扩散误差；可选邻居平滑，让像素沿用左、上邻居的颜色以便压缩。
- **收尾**（`src/lib.rs`）：oxipng `-o 2` 重压；原图不超过 256 种颜色、无损版本只大一点时用无损；比原图还大时保留原图。
- **命令行和浏览器逐字节相同**：并行的地方都按固定分块合并，结果不随线程数变化；`cbrt`、`powf` 用纯 Rust 的 `libm` 而不是平台的数学库；两组图 180 张都核对过。

## 基准测试

`bench/bench.py` 用同一批图片跑几种方案，记录体积，并用 libjxl 的 SSIMULACRA2 给每个输出打分：

| 方案 | 做了什么 |
|---|---|
| `oxipng` | 只做无损重编码，作为参照 |
| `pdT`、`pdauto` | `pngdeer --target T` / 自动画质，和命令行、浏览器完全相同（例如 `pd85`） |
| `pqN`、`pqN-dD` | `pngquant --quality 0-N`，抖动强度 D/100，再 `oxipng -o max` |
| `<工具>N`、`<工具>N-d0` | 其他开源量化器，N 色，带 / 不带 Floyd–Steinberg 抖动，再过同样的 oxipng：`im` ImageMagick、`ff` FFmpeg、`pil` Pillow、`exq` exoquant、`qte` quantette、`nq` NeuQuant、`qzr` quantizr（后四个用 `tools/quantlab`） |
| `tinypng` | TinyPNG API |

准备：

```bash
brew install pngquant oxipng jpeg-xl
```

```bash
cargo build --release
```

`ssimulacra2` 随 `jpeg-xl` 安装；脚本只用 Python 标准库。TinyPNG API key 在 [tinypng.com/developers](https://tinypng.com/developers) 申请（每月 500 次免费），复制 `.env.example` 为 `.env` 填入，或者设置 `TINYPNG_API_KEY`；没有 key 时跳过 `tinypng`。TinyPNG 的结果按原图内容缓存在 `bench/.cache/tinypng/`，同一张图只消耗一次额度。

运行：

```bash
python3 bench/bench.py
```

```bash
python3 bench/bench.py --images bench/images-holdout
```

默认方案是 `oxipng,pq100,pd70,pd75,pd80,pd85,pd90,pd95,pdauto,tinypng`。图片放在 `bench/images/`（第一层子目录是分类）和留出集 `bench/images-holdout/`（来源见各自的 SOURCES.md）。其中 `ui/` 的网站截图版权归各网站所有，不在仓库里，所以克隆下来的两组图分别是 94 张和 61 张，结果会和上面的表略有不同；图片会上传到 TinyPNG，不要放不能外传的图。常用参数：

- `--pipelines ...,im16,im32,...,im256,...`：其他工具各取几个颜色数连成曲线；需要 `brew install imagemagick ffmpeg`、Pillow，以及 `cargo build --release -p quantlab`
- `--pd-variants`、`--pd-preview-pixels`：传给 pngdeer 的 `--variants`、`--preview-pixels`
- `--small-side 256`：长边小于 256 的图放大后再打分（SSIMULACRA2 在缩小的尺度上比较，图太小时对边缘损伤不敏感）；`0` 关闭
- `-j 1`：一次只压一张图，耗时数字最干净；`--zopfli`、`--pq-speed 1`、`--oxipng-level`、`--images`、`--out`

每次运行写到 `bench/results/<时间>/`，`latest` 指向最近一次：`report.md`（总览、分类明细、和 TinyPNG 的同画质对比、逐图对比）、`results.csv`（逐图逐方案的原始数据）和各方案的输出文件。原图会先去掉色彩空间块（gAMA/cHRM/sRGB/iCCP）再交给各工具，打分只比较像素；带透明的图分别叠在深色和浅色背景上打分，取较差的分数。`experiments/palette-feedback/butteraugli_check.py` 用 butteraugli 给一次运行的输出重新打分。

### 盲测

```bash
python3 bench/blind.py
```

然后打开 <http://127.0.0.1:8765/>。页面读 `bench/results/latest`，每张图拿 TinyPNG 的输出和体积最接近的 pngdeer 输出，随机标成 A、B，投票后才揭晓。`1` / `2` 切换 A、B，按住空格看原图，`F` 来回闪切，`Z` 缩放（100% 是一个图片像素对一个屏幕物理像素），`G` 换背景，`←` / `↓` / `→` 投票，`⏎` 下一张，`R` 看结果。投票存在 `bench/results/<那次运行>/blind/`。`--match quality` 改成取分数不低于 TinyPNG 的最小输出，`--challenger pq` 换成 pngquant，`--results DIR` 评别的运行。

## 目录

- `src/`：压缩核心（库）和命令行
- `web/`：浏览器版页面（项目主页）、Web Worker、WebAssembly 包装（`web/wasm/`）和 HeyDeer 标志（`web/brand/`）
- `vendor/fast-ssim2/`：打了三个提速补丁、分数不变的 fast-ssim2（BSD-2-Clause，见其中的 PATCHES.md）
- `bench/`：基准测试、盲测和两组测试图
- `tools/quantlab/`：给基准测试调用其他开源量化器
- `experiments/palette-feedback/`：研究记录（FINDINGS.md）和打分工具

## 许可

MIT 或 Apache-2.0，任选其一（`LICENSE-MIT`、`LICENSE-APACHE`）。第三方代码的许可见 `web/THIRD_PARTY_LICENSES.txt`。
