"""sv_decision 判别逻辑单测——只覆盖「阈值/比对逻辑正确」。

刻意 import 零依赖的 sv_decision（不碰 funasr/torch/numpy），开发机直接跑：
  cd deploy/sv && python3 test_sv_decision.py

真实「音色相近误识别」的 FAR/FRR 必须用以琳本人 + 同伴的真实音频跑 /verify
验，单测覆盖不了——这里只保证：给定 cos score，放行/拒绝的边界对。
"""
import sv_decision


def test_default_threshold_tightened_to_0_5():
    """回归断言：default 阈值是 0.5（原 0.3 对同性同口音相近音色过松）。"""
    assert sv_decision.DEFAULT_THRESHOLD == 0.5


def test_confusable_imposter_band_rejected():
    """音色相近的他人——score 落 0.3~0.5 混淆带——必须被拒。

    issue d22400a1 核心：0.3 阈值下 0.3-0.5 全过；0.5 阈值下全拒。"""
    for score in (0.30, 0.38, 0.45, 0.49):
        assert sv_decision.decide_match(score) is False, f"score={score} 应拒"


def test_clear_owner_passes():
    """owner 本人 CAM++ same-speaker score 通常 0.6-0.8，必须放行。"""
    for score in (0.50, 0.62, 0.75, 0.88):
        assert sv_decision.decide_match(score) is True, f"score={score} 应放行"


def test_obvious_stranger_rejected():
    """音色差异大的陌生人 score ≈ 0.0，照拒不误。"""
    for score in (-0.1, 0.0, 0.12, 0.25):
        assert sv_decision.decide_match(score) is False, f"score={score} 应拒"


def test_boundary_equal_passes_just_below_rejects():
    """边界：恰好等于阈值放行（>=），略低于拒绝。"""
    t = sv_decision.DEFAULT_THRESHOLD
    assert sv_decision.decide_match(t) is True
    assert sv_decision.decide_match(t - 1e-6) is False


def test_explicit_threshold_override_respected():
    """显式传 threshold 覆盖 default（部署机实测微调路径）。"""
    assert sv_decision.decide_match(0.35, threshold=0.3) is True
    assert sv_decision.decide_match(0.35, threshold=0.6) is False


if __name__ == "__main__":
    # 无 pytest 也能跑：手动收集 test_ 函数。
    import sys

    tests = [v for k, v in sorted(globals().items())
             if k.startswith("test_") and callable(v)]
    failed = 0
    for t in tests:
        try:
            t()
            print(f"  PASS  {t.__name__}")
        except AssertionError as e:
            failed += 1
            print(f"  FAIL  {t.__name__}: {e}")
    print(f"\n{len(tests) - failed}/{len(tests)} passed")
    sys.exit(1 if failed else 0)
