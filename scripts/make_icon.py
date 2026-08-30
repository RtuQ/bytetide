"""生成 ByteTide(字节潮) 应用图标源图（1024x1024 RGBA，透明圆角）。

设计：深色圆角方底（#0f172a -> #0b1220 纵向渐变）+ #334155 细描边；
      圆角方波波形（串口信号/示波器记号）横贯画面，蓝->青->绿渐变
      （#3b82f6 -> #22d3ee -> #34d399，呼应 accent->RX 的数据流向），
      波形带柔和辉光；末端琥珀脉冲点（#fbbf24 = TX/活动信号）。
      与 TitleBar.vue 品牌图标同一记号，任务栏/标题栏品牌一致。
"""
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

S = 1024
TOP = (15, 23, 42)      # --bg-surface #0f172a
BOT = (11, 18, 32)      # --bg-base   #0b1220
BORDER = (51, 65, 85)   # --border-strong #334155
BLUE = (59, 130, 246)   # --accent #3b82f6
CYAN = (34, 211, 238)   # #22d3ee 蓝绿过渡
GREEN = (52, 211, 153)  # --rx #34d399
AMBER = (251, 191, 36)  # --tx #fbbf24

PAD = 40
R = 188

# 方波折点：低起，等距周期，末端收在高电平并缀琥珀脉冲点（含描边/圆点整体居中于 512,512）
WAVE = [
    (186, 647), (346, 647), (346, 377), (506, 377),
    (506, 647), (666, 647), (666, 377), (826, 377),
]
TIP = WAVE[-1]
STROKE = 68
RAD = STROKE // 2

img = Image.new('RGBA', (S, S), (0, 0, 0, 0))


def _round_mask():
    m = Image.new('L', (S, S), 0)
    ImageDraw.Draw(m).rounded_rectangle(
        [PAD, PAD, S - PAD, S - PAD], radius=R, fill=255
    )
    return m


# 1. 纵向渐变底（1 像素宽列再水平拉伸）
col = Image.new('RGBA', (1, S))
px = col.load()
for y in range(S):
    t = y / (S - 1)
    px[0, y] = tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOT)) + (255,)
img.paste(col.resize((S, S)), (0, 0), _round_mask())


# 2. 细描边
ImageDraw.Draw(img).rounded_rectangle(
    [PAD, PAD, S - PAD, S - PAD], radius=R, outline=BORDER + (255,), width=6
)

# 3. 水平渐变（蓝->青->绿），1 像素行再竖向拉伸
def lerp(c1, c2, t):
    return tuple(round(a + (b - a) * t) for a, b in zip(c1, c2))


gx0, gx1 = WAVE[0][0], TIP[0]
row = Image.new('RGBA', (S, 1))
rp = row.load()
for x in range(S):
    t = min(max((x - gx0) / (gx1 - gx0), 0.0), 1.0)
    c = lerp(BLUE, CYAN, t * 2) if t < 0.5 else lerp(CYAN, GREEN, (t - 0.5) * 2)
    rp[x, 0] = c + (255,)
grad_h = row.resize((S, S))

# 4. 波形描边蒙版：joint='curve' 外侧圆角/内侧直角（数字方波感），端点圆头
stroke_mask = Image.new('L', (S, S), 0)
sd = ImageDraw.Draw(stroke_mask)
sd.line(WAVE, fill=255, width=STROKE, joint='curve')
for x, y in (WAVE[0], TIP):
    sd.ellipse([x - RAD, y - RAD, x + RAD, y + RAD], fill=255)

# 5. 波形辉光（同几何加粗模糊，45% 透明度，裁进圆角内）
round_mask = _round_mask()
glow_alpha = stroke_mask.filter(ImageFilter.GaussianBlur(50))
glow_alpha = ImageChops.darker(glow_alpha.point(lambda v: v * 45 // 100), round_mask)
glow = grad_h.copy()
glow.putalpha(glow_alpha)
img.alpha_composite(glow)

# 6. 波形主体
img.paste(grad_h, (0, 0), stroke_mask)

# 7. 末端琥珀脉冲点 + 暖色光晕
tip_glow = Image.new('L', (S, S), 0)
ImageDraw.Draw(tip_glow).ellipse(
    [TIP[0] - 80, TIP[1] - 80, TIP[0] + 80, TIP[1] + 80], fill=255
)
tip_alpha = ImageChops.darker(
    tip_glow.filter(ImageFilter.GaussianBlur(38)).point(lambda v: v * 32 // 100),
    round_mask,
)
amber_layer = Image.new('RGBA', (S, S), AMBER + (0,))
amber_layer.putalpha(tip_alpha)
img.alpha_composite(amber_layer)

d = ImageDraw.Draw(img)
d.ellipse([TIP[0] - 46, TIP[1] - 46, TIP[0] + 46, TIP[1] + 46], fill=AMBER + (255,))

# 输出到脚本所在目录（与源图同处 scripts/，不污染仓库根目录）
out = str(Path(__file__).with_name('brand-icon.png'))
img.save(out)
print('wrote', out, img.size, img.mode)
