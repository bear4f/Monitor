# Monitor UI 复刻规格

状态：Phase 1 冻结  
日期：2026-09-06

## 1. 权威性与测量方法

本文件不是重新设计稿。五张截图是布局、密度和层级的最高权威：

1. `docs/reference/01-overview.png`
2. `docs/reference/02-node-resource-top.png`
3. `docs/reference/03-node-resource-bottom.png`
4. `docs/reference/04-node-latency.png`
5. `docs/reference/05-admin-nodes.png`

截图没有 DOM/CSS 原值，以下像素为按图像边界测得的实现基线。Phase 4/5/6 必须在相同 viewport 生成截图叠图复核，允许为匹配原图做约 ±2px 修正；不得借“修正”为名改变内容、布局层级或视觉风格。

禁止渐变、Glow、glass、背景图、巨大标题、夸张阴影、营销布局、彩虹图表和装饰动画。LuminaPlus 中与这些禁项相关的样式不采用。

## 2. 全局设计基础

### 2.1 Container

| Surface | max width | 大屏水平 padding | 居中规则 |
| --- | --- | --- | --- |
| Public overview/detail | 2048px | 64px | `width:100%; margin-inline:auto` |
| Admin header/content | 1392px | 16px（max-width 内） | 1660px 图中约 134px 外侧留白 |

Public 的 2048px max width 是为保持参考图中约 470px 的四列卡片；超过 2048px 时不继续拉宽卡片。页面背景可铺满 viewport。

### 2.2 字体

```css
font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui,
  "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", sans-serif;
font-variant-numeric: tabular-nums;
```

不加载外部字体。正文默认 `16px/1.5`，字间距 0；数字不使用独立字体。

| Role | size / line-height | weight |
| --- | --- | --- |
| Site name | 22px / 28px | 700 |
| Page/node title | 24px / 32px | 650–700 |
| Card node title | 22px / 28px | 650 |
| Summary primary value | 28px / 34px | 700 |
| Section/chart title | 17px / 24px | 600 |
| Body/value | 17px / 24px | 400–600 |
| Label/muted | 15–16px / 22px | 400–500 |
| Badge/table | 15–16px / 20px | 500–600 |
| Small caption | 13–14px / 20px | 400–500 |

不要用全大写英文 label，也不要人为扩大 tracking。中英文混排用系统默认字距。

### 2.3 浅色 token

接近 shadcn neutral，但以截图的白底黑灰为准：

| Token | Value | 用途 |
| --- | --- | --- |
| `--background` | `#fafafa` | 页面外层画布 |
| `--foreground` | `#171717` | 主文本/深色进度 |
| `--card` | `#ffffff` | 卡片、header、table、tooltip |
| `--card-foreground` | `#171717` | 卡片文本 |
| `--muted` | `#f3f3f3` | 轨道、选中 sidebar、skeleton |
| `--muted-foreground` | `#737373` | 次级文本、axis |
| `--border` | `#dedede` | 卡片/table/控件边框 |
| `--input` | `#d4d4d4` | Input 与 checkbox 边框 |
| `--primary` | `#171717` | 激活 Tab、主按钮 |
| `--primary-foreground` | `#fafafa` | 主按钮文字 |
| `--secondary` | `#f1f1f1` | 次按钮/hover |
| `--secondary-foreground` | `#262626` | 次按钮文字 |
| `--destructive` | `#dc2626` | 仅删除/严重错误 |
| `--ring` | `#737373` | focus ring |
| `--progress-track` | `#ececec` | 指标进度背景 |
| `--progress-fill` | `#111111` | 指标进度前景 |
| `--chart-line` | `#404040` | 单 series 图 |
| `--chart-fill` | `rgba(64,64,64,.13)` | CPU/内存/磁盘面积 |
| `--chart-grid` | `#d4d4d4` | 横向虚线 grid |
| `--online` | `#171717` | 在线小圆点；不做绿色大 Badge |
| `--offline` | `#b45353` | 离线文字/点，低饱和 |

### 2.4 深色 token

深色使用炭灰而非纯黑大面积背景：

| Token | Value |
| --- | --- |
| `--background` | `#111113` |
| `--foreground` | `#f4f4f5` |
| `--card` | `#18181b` |
| `--card-foreground` | `#f4f4f5` |
| `--muted` | `#222225` |
| `--muted-foreground` | `#a1a1aa` |
| `--border` | `#303033` |
| `--input` | `#3f3f46` |
| `--primary` | `#e4e4e7` |
| `--primary-foreground` | `#18181b` |
| `--secondary` | `#27272a` |
| `--secondary-foreground` | `#e4e4e7` |
| `--destructive` | `#f87171` |
| `--ring` | `#71717a` |
| `--progress-track` | `#303033` |
| `--progress-fill` | `#d4d4d8` |
| `--chart-line` | `#d4d4d8` |
| `--chart-fill` | `rgba(212,212,216,.11)` |
| `--chart-grid` | `#3f3f46` |
| `--online` | `#e4e4e7` |
| `--offline` | `#d97777` |

Dark card 不加白色顶光或彩色边缘；主要靠 `#111113 / #18181b / #303033` 三层区分。颜色切换只做一次 class/token 变更，不给所有元素添加 transition。

### 2.5 边框、圆角、阴影

- 默认控件 radius：8px；Button 8–10px；Badge 999px。
- Summary/Node card radius：18px 桌面、14px 手机。
- Admin table radius：14px。
- Card border：1px solid `--border`。
- 浅色 card shadow：`0 1px 2px rgba(0,0,0,.08), 0 2px 5px rgba(0,0,0,.04)`。
- 深色 card/table 不使用外投影；只保留 border。
- Dialog 可用 `0 16px 40px rgba(0,0,0,.18)`，这是唯一明显浮层阴影。

## 3. 公共 Header

| 项目 | Overview/detail desktop | <768px |
| --- | --- | --- |
| 高度 | 88px | 64px |
| border | 1px bottom | 同 |
| 水平 padding | 与 public container 相同 | 16px |
| site name | 22/28, 700 | 19/24, 700 |
| 右侧按钮 | 36px 高，gap 26px | icon button 36px，登录文字可保留 |
| icon | 21–22px, stroke 2 | 20px |

Header 白/炭灰实色，不 sticky 时也不得添加透明/blur。参考图 03 的顶部 header 是滚动后仍可见状态，因此实现为 `position: sticky; top:0; z-index`，背景必须完全不透明。

## 4. 逐图分析：01 Overview

文件：`01-overview.png`  
Viewport：2048×932。

### 4.1 几何

- Header 底边约 y=85–88。
- 内容左边约 x=63，右边约 x=1997，对应 64px 水平 padding。
- Summary top 约 y=109：header 后 20–22px。
- Summary 四列等宽，单卡约 470px，column gap 18px，高约 160px。
- Node grid top 约 y=297，距 summary bottom 约 28px。
- Node 四列等宽，与 summary 列线完全对齐；column/row gap 18px。
- 首行 Node card 约 405px 高；内容 padding 24px。
- 页面不使用中心窄栏，也不在 summary 上方添加标题。

### 4.2 Summary cards

- radius 18px；1px border；轻 shadow。
- 内 padding 18px；内容采用三段纵向分布，不能把数字垂直居中成营销卡。
- Label：16px/22，500，muted；左侧 icon 18px。
- 主值：28px/34，700；最忙节点/节点名称 caption 16px。
- 今日流量和实时网速为两列；两列数值 baseline 对齐。
- 总流量 label 14–15px；总值 18px/24。
- Sparkline 区约 42px 高，两条 1–1.5px 中性线，无 axis/grid/点/动画。

### 4.3 Node card

- Header 首行高度约 31px：名称 22px/28，地区 Badge 紧随；在线 Badge 靠右。
- 地区 Badge：最小 38×30px，padding 0 10px，font 15–16，1px border。
- 在线 Badge：约 168×31px（随文案），padding 0 11px；dot 8px；不填充绿色背景。
- 第二行 margin-top 4px；OS 与到期都是 17px/24 muted，左右对齐，单行截断。
- 资源区 margin-top 22px，2 列、column gap 22px、row gap 22px。
- Metric label/value 同行：17px/22；value weight 600。
- Progress：8px 高，radius 999px，label 后 8px；黑灰 fill，无渐变。
- Progress detail：margin-top 8px，17px/24 muted。
- Load 没有独立进度文案，显示 `0.00 0.00 0.00`。
- Divider：资源区后 22px，1px；footer top padding 20px。
- Footer 2×2：实时第一行主色，累计第二行 muted；箭头 icon 15–16px，列 gap 与资源区一致。
- Offline：card opacity 建议 .58–.68；在线 Badge 改低饱和红/灰，最后数值原位保留。

### 4.4 首页 skeleton

首屏 HTML/CSS 立即渲染 header 和同尺寸 card shell。Skeleton 只用 muted 实色块与低幅 opacity pulse；不改变 card 高度，不用 shimmer gradient。

## 5. 逐图分析：02 Node resource top

文件：`02-node-resource-top.png`  
Viewport：2047×941。

### 5.1 顶部信息

- Header 约 89px 高；内容 x=63 起。
- 节点标题行 top 约 117px，高 33px；title 24px/32。
- 地区、在线、Agent Badge 高约 31px，彼此 gap 10–12px。
- 信息区 top 约 179px，三等列；列起点约 x=63、718、1374。
- 每列两组信息，组间约 18–20px；label 16px muted，value 20px/27、500。
- 内容分隔线约 y=317；信息区到分隔线留足 22–24px，不包 Card。

### 5.2 Tabs 与 range

- 主 Tab top 约 y=343；高度 35px；两个 trigger 间 gap 4–6px。
- 激活项近黑实色、白字、radius 10px、水平 padding 14px。
- 非激活项无背景、muted text。
- Range row top 约 389px；trigger 高 34px，水平 padding 14px，四项顺序 1h/6h/24h/7d。
- 主 Tab 与 range 之间约 10px；range 与第一图标题约 23px。

### 5.3 Resource chart

- CPU section title 约 y=450；17px/24、600、muted foreground。
- plot 左侧为约 104px axis gutter，右侧 58px；主 plot 约 175–190px 高。
- 一个完整 resource chart block（title、plot、x axis、下间距）约 250–270px。
- CPU Y 轴使用百分比；自动范围仍从 0 开始，刻度保持 4–5 条。
- 单线 `--chart-line` 1.5px；CPU/内存/磁盘填 `--chart-fill`；points hidden。
- x axis 16px muted，整点为主刻度；图表不包 Card、不画 plot 外框。

### 5.4 Tooltip

截图 tooltip 约 185×126px：实色 card、1px border、radius 0–4px、padding 14px、无明显 shadow。时间 16px muted；数值 17px foreground；series 行 gap 8px。Hover cursor 是 1px 实线，交点 8px dot + 2px card-color border。

## 6. 逐图分析：03 Node resource bottom

文件：`03-node-resource-bottom.png`  
Viewport：2047×938；这是同一详情页滚动后的下半段，不是独立页面。

### 6.1 连续节奏

- Sticky header 仍占顶部约 90px，并覆盖滚动内容；不能让图表穿透 header。
- 内存图、网络速率图、硬盘图的左/右 plot 边界完全一致。
- 各 chart section 之间约 62–76px 视觉空隙；这是大画布留白，不应改成 Card gap。
- Network title 约 y=381，plot 约 y=420–598；Disk title约 y=685，plot约 y=721–899。

### 6.2 Network chart

- RX/TX 两条线，均为中性深/中灰；线宽分别约 1.5px、1.2px，可用透明度和 dash 辅助区分。
- 不使用蓝/绿方向色填满图表；只有很浅的灰 fill 可选，不能遮盖另一条线。
- Y axis 自适应并格式化 `B/s, KB/s, MB/s`；0 必须显示 `0 B/s`。
- Tooltip 顺序固定：时间、上传、下载，与截图一致；箭头不必放入 tooltip。

### 6.3 Memory/Disk chart

- 标题显示总量，例如“内存 · 965 MB”“硬盘 · 19.6 GB”。
- Y tick 使用绝对 bytes 格式而非百分比；0 为 `0 B`。
- 接近水平的 series 仍保留浅灰面积，不把 y 轴自动缩放到夸张波动；范围为 0..当前 total。

## 7. 逐图分析：04 Node latency

文件：`04-node-latency.png`  
Viewport：2048×942。

### 7.1 控件

- 详情头部与图 02 完全相同。
- “网络延迟”主 Tab 激活；range row 同尺寸。
- 控制顺序固定为 1h、6h、24h、7d、削峰；checkbox 18×18px，label 16px，距 7d 约 18–24px。
- 控制区到 plot top 约 27px。

### 7.2 Ping plot

- plot 左 gutter 约 112px，右 gutter约 68px。
- 主 plot 约 355–380px 高；含 x axis/底部 navigator 的 chart 区约 420px。
- Y axis 以 ms 显示，约 4–5 个 tick；横向 1px `4 4` dashed grid。默认不画垂直 grid。
- 最多 6 series，1.25–1.6px；points hidden；null gap 不连接。
- 颜色低饱和，且必须配合线型区分：solid、`6 4`、`2 4`、`10 4 2 4` 等。禁止红橙黄绿蓝紫彩虹映射。
- 底部 navigator/选择条高约 34px，使用浅灰 track 和深灰 2px top line；它只做可视窗口，不增加另一套数据。
- Legend 在图表下居中，outline Badge 高 36px、radius 10px、gap 8px；线型样本 20px，文字 16px。

建议浅色六线：`#3f3f46, #6b6b70, #525a57, #85827c, #5f646b, #918985`；深色使用对应的 `#d4d4d8, #a1a1aa, #b4bbb7, #8f8c86, #a8adb5, #aaa19d`。这些颜色不是指标状态色。

### 7.3 削峰显示

- 只处理当前 viewport 内每条 series 的非 null 副本；数据库/API 不变。
- 少于 20 个有效点时不裁剪。
- 建议 cap：`max(p95, median + 6×MAD, median + 20ms)`；显示值为 `min(raw, cap)`。
- Tooltip 始终显示 raw；发生裁剪时追加简短“显示已削峰”标记。
- null 经过削峰仍为 null，不能插值或变 0。

## 8. 逐图分析：05 Admin nodes

文件：`05-admin-nodes.png`  
Viewport：1660×948。

### 8.1 Frame

- Admin header 高约 70px；底部 1px border。
- Header/content 共用约 1392px centered container；图中左边约 x=135。
- Header 左侧“Monitor”17–18px/24、700，“后台”13–14px muted，gap 10px。
- Header 右侧状态面板、主题、退出；icon 17–18px，item gap 28px。
- Main top 约 96px；sidebar 约 194px；sidebar 与 content gap 26px。

### 8.2 Sidebar

- 四项顺序固定：节点、延迟监控、主题、设置。
- item 约 194×40px，padding 0 13px，radius 9px；row gap 6px。
- icon 17–18px；文字 15–16px/22、500。
- active 使用 `--muted` 实色，不用彩色竖线、渐变或 shadow。

### 8.3 Toolbar 与 table

- 添加节点按钮位于主区右上，约 117×40px，radius 10px，icon 16px，font 15–16/600。
- Toolbar bottom 到 table top 约 18px。
- Table 宽约 1153px；border 1px；radius 14px；轻 shadow；overflow hidden。
- Header row 45px；data row 64–65px；单元格水平 padding 12–16px。
- Table font 15–16px；header 600；body 500；辅助容量/额度用 muted。
- 首列 drag handle 16px，handle 与名称 gap 14px。
- 状态 Badge 约 47×27px，深色实底/白字；离线用 muted outline，不用大片红。
- 操作 icon 18px，点击热区至少 36×36px，视觉 gap 4–6px；删除 icon 才使用 destructive。
- IP 可两行显示 IPv4/IPv6，最多 2 行并截断；不扩大 row 高。

操作固定为三组，不增加“更多管理”：

1. 安装 Dropdown：Agent x86_64/arm64 下载链接；节点创建后可直接复制安装命令。旧明文 token 不可恢复，既有节点再次生成命令时必须先确认调用 `rotate-token` 会令当前 Agent 失效。
2. 编辑：打开节点字段 Dialog。
3. 删除：打开不可恢复确认 Dialog。

不提供总流量清零、远程升级、远程配置或命令入口。

Table column 基线：名称 15%、IP 27%、状态 9%、流量 15%、价格 9%、到期 13%、操作 12%。操作列右对齐。小屏保持 `min-width: 900px` 后横向滚动，不把每行改造成装饰性 card。

## 9. Primitive 尺寸

| Primitive | 默认规格 |
| --- | --- |
| Button | 36px 高；admin primary 40px；px 12–16；radius 8–10 |
| Icon button | 36×36px，icon 18px |
| Input/Select-like Dropdown trigger | 38px 高；px 12；radius 8；font 15 |
| Badge | 30–32px 高；px 10–11；pill |
| Dialog | width min(480px, calc(100vw - 32px))；radius 14；padding 24 |
| Tabs trigger | 34–35px 高；px 13–14；radius 9–10 |
| Switch | 34×20px；thumb 16px |
| Checkbox | 18×18px；radius 4 |
| Tooltip | padding 8px 10px；font 13–14；radius 6；chart tooltip 另按截图 |
| Dropdown menu | min-width 180px；item 34px；radius 10；1px border |

所有交互目标最小 36px；纯视觉 icon 设 `aria-hidden`。Focus visible 为 2px ring + 2px offset，不因追求截图而移除键盘状态。

## 10. 响应式矩阵

截图只提供桌面，以下规则保持相同信息顺序与密度，不另造视觉风格。

| Viewport | Public padding | Summary | Node cards | Detail info | Admin |
| --- | --- | --- | --- | --- | --- |
| `>=1600` | 64px | 4 列 | 4 列 | 3 列 | 194px sidebar + table |
| `1280–1599` | 32–40px | 4 列 | 3 列 | 3 列 | 184px sidebar + table |
| `1024–1279` | 24px | 2 列 | 2 列 | 3 列 | 168px sidebar + horizontally scrollable table |
| `768–1023` | 20px | 2 列 | 2 列 | 2 列 | sidebar 变为主区上方水平四项 nav；table 横向滚动 |
| `<768` | 16px | 1 列 | 1 列 | 1 列 | 水平 nav 可滚动；toolbar 与 table 分行；table 横向滚动 |

断点使用 CSS `1600px / 1280px / 1024px / 768px`，不直接依赖 Tailwind 默认 `2xl=1536`；在 Tailwind theme 中声明项目值。

### 10.1 Card 行为

- Node grid 使用 `minmax(0,1fr)`，不能设置导致 1280px 三列溢出的固定 470px min width。
- ≥1600 时 public max width 把四列控制在参考图接近 470px。
- 768–1023 两列时缩小 node card padding 到 18–20px，但内部指标仍保留两列；若单卡内容宽低于 340px，再把指标变一列。
- <768 node card 内部优先保持两列；viewport <420px 时 column gap 降至 14px，文字使用 ellipsis，不缩小到 13px 正文。
- Summary 在 1280 保持四列，因为其内容比 Node card 短；1024 降为 2 列。

### 10.2 Detail 行为

- 标题/Badge 可换行，节点名独占优先；Badge 不压缩。
- 1024 仍三列信息；768–1023 两列；手机单列。
- Tabs/range 横向可滚动但隐藏滚动条，顺序不变；不能改成 Dropdown。
- Resource chart desktop plot 190px；768–1023 为 180px；手机为 170px，axis gutter 缩至 58–68px。
- Ping desktop plot 370px；平板 320px；手机 280px。Legend 可换行，保持 Badge。
- 手机 tooltip 宽不超过 plot，优先固定到触点另一侧；不覆盖整屏。

### 10.3 Admin 行为

- ≥1024 保持 screenshot sidebar/table 架构。
- <1024 只把同四项 sidebar 变为水平导航，不新增 hamburger/drawer。
- Table 使用真实 `<table>` + overflow container；拖动 handle 在触控上提供明确长按/键盘排序，但不增加独立排序页面。
- Dialog 手机宽 `calc(100vw - 32px)`，表单单列；桌面 label/input 仍单列堆叠以减少错误。

## 11. 图表统一规格

| 项目 | Resource | Ping |
| --- | --- | --- |
| uPlot points | hidden | hidden |
| line | 1.5px | 1.25–1.6px |
| fill | CPU/memory/disk 13%；network ≤6% | none |
| grid | 仅水平，1px dashed 4/4 | 同 |
| axes font | 15–16px system muted | 同 |
| animation | none | none |
| gap | null，`spanGaps:false` | null，`spanGaps:false` |
| cursor | 1px vertical + point | 同 |
| legend | hidden，tooltip 标识 | 底部 Badge legend |

uPlot CSS 和代码只在详情页 lazy chunk。ResizeObserver 只更新尺寸，不在每个 snapshot tick 重建 chart instance；数据变化调用 `setData`。

## 12. 内容与格式

- Bytes 使用 IEC 1024 换算，但 UI 文案沿截图写 `KB/MB/GB/TB`，最多 2 位小数并去掉无意义尾零。
- Rate 追加 `/s`；0 显示 `0 B/s`。
- CPU：低于 10% 保留 1 位或截图需要的 `0.0%`；不显示 4 位小数。
- Load 固定两位。
- Uptime：最多两个最大单位，如“28 天 9 小时”。
- 到期：首页“60 天后到期”；已过期“已过期 3 天”；无到期 `∞`。
- Price：0 为“免费”；币种用 Intl 格式化；周期映射月付/季付/半年付/年付/两年付/自定义。
- Region code 保持大写两字母，不加载旗帜图片。
- 节点/站点超长名称单行 ellipsis；tooltip 提供完整文本。

## 13. 主题与可访问性

- 三态选择：light/dark/system。用户 localStorage 选择优先；无选择时使用 snapshot 的 `theme_default`。
- 首屏在 React 前用固定、极小的主题启动脚本读取 localStorage 并设置 class；CSP 只放行该脚本的构建期 SHA-256 hash，不允许其他 inline script。system 用 `matchMedia`/`color-scheme`。
- `html` 同步 `color-scheme: light dark`，原生 input/dialog 随主题。
- 文本与背景目标 WCAG AA；muted 小字不能低于 4.5:1。
- 所有图表有可读 `aria-label` 和相邻 summary；不为了无障碍新增视觉卡片。
- `prefers-reduced-motion` 下关闭 skeleton pulse 和所有非必要 transition。

## 14. 视觉验收

Phase 4/5/6 每页在以下 viewport 截图：2048×932、1660×948、1600×900、1280×800、1024×768、768×1024、390×844；light/dark 各一份。

验收顺序：

1. Container、header、栅格和卡片边界；
2. 字号、行高、文本 baseline；
3. border/radius/shadow；
4. control/badge/progress 尺寸；
5. chart plot/axis/tooltip；
6. 颜色与 dark；
7. responsive overflow、ellipsis、keyboard focus。

不得用动画、渐变、模糊背景或新增内容掩盖尺寸误差。
