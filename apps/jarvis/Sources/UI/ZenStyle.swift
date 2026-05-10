import SwiftUI

/// 禅意药丸 GUI 设计 token——颜色 / 几何 / 动效常量集中在这。
///
/// 设计语言：东方水墨留白。胶囊本身只占 40%，60% 留白；元素全用极细 1.5px 线条；
/// 主调淡墨青，朱砂只在高电平 / 警示时点睛。light/dark 双套配色由 SwiftUI 自动切换。
enum ZenStyle {
    // MARK: - 颜色（light / dark 两套）

    /// 宣纸底——胶囊主背景的色调（NSVisualEffectView 之上还要叠 8% 这色）。
    static let paperLight = Color(red: 0xFA / 255.0, green: 0xF7 / 255.0, blue: 0xF1 / 255.0)
    static let paperDark = Color(red: 0x1F / 255.0, green: 0x1C / 255.0, blue: 0x1A / 255.0)

    /// 淡墨青——主点缀色（idle 圆点、listening 波形低位、speaking 波形）。
    static let inkTealLight = Color(red: 0x6E / 255.0, green: 0x88 / 255.0, blue: 0x96 / 255.0)
    static let inkTealDark = Color(red: 0x9E / 255.0, green: 0xB4 / 255.0, blue: 0xBE / 255.0)

    /// 朱砂——高电平 / 警示色（listening 波形尖端、ack 横扫起势）。
    static let cinnabarLight = Color(red: 0xC0 / 255.0, green: 0x4F / 255.0, blue: 0x45 / 255.0)
    static let cinnabarDark = Color(red: 0xD9 / 255.0, green: 0x6E / 255.0, blue: 0x64 / 255.0)

    /// 笔触灰——1px hairline 描边色。
    static let strokeLight = Color(red: 0xD4 / 255.0, green: 0xCF / 255.0, blue: 0xC4 / 255.0)
    static let strokeDark = Color(red: 0x3A / 255.0, green: 0x36 / 255.0, blue: 0x32 / 255.0)

    // MARK: - 几何

    /// 胶囊外尺寸——夹在 dock 上方需要紧凑，不能占太大屏幕底部空间。
    static let capsuleWidth: CGFloat = 160
    static let capsuleHeight: CGFloat = 32
    /// 圆角 = 高度一半——半圆形端点。
    static let capsuleCornerRadius: CGFloat = capsuleHeight / 2
    /// 描边线宽——1px hairline。@2x 屏物理 0.5pt。
    static let strokeWidth: CGFloat = 1
    /// dock 上方留 12px 间距——避免吸到 dock 触发 dock magnification。
    static let dockGap: CGFloat = 12

    /// 60% 留白——元素只占中央 40%。给波形 / 三点缓行 / 圆点用。
    static let contentInsetRatio: CGFloat = 0.30 // 两侧各 30%，中央 40%

    /// idle 圆点直径。
    static let idleDotDiameter: CGFloat = 6

    /// listening 波形条数 + 宽度 + 间距。
    static let waveBarCount = 18
    static let waveBarWidth: CGFloat = 1.5
    static let waveBarSpacing: CGFloat = 1.5

    /// sending/waiting 三点直径。
    static let waitingDotDiameter: CGFloat = 4
    static let waitingDotSpacing: CGFloat = 8

    // MARK: - 动效周期

    /// idle 呼吸周期（秒）——慢柔光。
    static let breathePeriod: Double = 2.4
    /// 三点缓行周期。
    static let waitingPeriod: Double = 1.6
    /// ack 横扫总时长——earcon 200ms 同步，淡入淡出各 100ms。
    static let ackSweepDuration: Double = 0.2
    /// listening 波形 wobble 频率——配合 audioLevel 给基本扰动。
    static let waveBaseFrequency: Double = 8

    // MARK: - 动态色——按 colorScheme 切换

    static func paper(_ scheme: ColorScheme) -> Color {
        scheme == .dark ? paperDark : paperLight
    }
    static func inkTeal(_ scheme: ColorScheme) -> Color {
        scheme == .dark ? inkTealDark : inkTealLight
    }
    static func cinnabar(_ scheme: ColorScheme) -> Color {
        scheme == .dark ? cinnabarDark : cinnabarLight
    }
    static func stroke(_ scheme: ColorScheme) -> Color {
        scheme == .dark ? strokeDark : strokeLight
    }
}
