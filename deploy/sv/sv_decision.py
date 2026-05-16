"""SV 判别逻辑——纯函数，零依赖（不 import numpy/torch/funasr）。

抽出来单独成模块的原因：sv_server.py 一 import 就拉整个 funasr/torch 栈，
开发机上跑不动单测。把「给定 cos score → 放行/拒绝」这个决策抽到这里，
测试只 import 本模块即可，在任何机器上都能验逻辑正确性。

issue d22400a1：音色相近的他人被误判为 owner。根因是阈值 0.3 过松——
CAM++ 对 same-gender/same-accent 的他人 cos 常落 0.3~0.5，并非注释假设的 ≈0.0。
0.3 把这个混淆带全放行了。提到 0.5 把混淆带划进拒绝侧。

阈值取值依据：CAM++（speech_campplus_sv_zh-cn_16k-common）same-speaker cos
集中在 0.6~0.8；不同说话人里「音色相近」是 0.3~0.5，「明显不同」≈0.0。
0.5 让 owner 仍有 0.1+ headroom，同时把相近音色挡在外面。
偏严带来的代价是 owner 偶发被拒（FRR 略升）——但本旁路 fail-open（SV
故障/未注册放行），且误拒只是「再喊一次」，远比误放行陌生人安全。
"""

# 默认 verify 阈值。原 0.3 过松（见 module docstring），提到 0.5。
# 可被环境变量 SV_THRESHOLD 覆盖（部署机按真实 FAR/FRR 实测微调）。
DEFAULT_THRESHOLD = 0.5


def decide_match(score: float, threshold: float = DEFAULT_THRESHOLD) -> bool:
    """cos 相似度 → 是否判定为 owner。

    `score >= threshold` 放行。边界恰等放行（owner 容错优先于偏严）。
    """
    return score >= threshold
