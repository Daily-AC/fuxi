import XCTest
@testable import Jarvis

// `isWakeKeywordHit` 纯函数单测——LocalWakeFallback 用它判断 SFSpeechRecognizer 的
// 转写是否命中唤醒词。
//
// 规则（team-lead 拍板，B-tightened，只看左侧）：
//   keyword 必须 substring AND 紧邻左侧字符 ∈ {string 起始, ASCII, 标点, 空白, 非中文 Unicode}。
//   右侧不做约束。
//
// 理由：呼语前一般有停顿/句首/非中文邻接；右侧的语气词「玄女啊」「玄女帮我...」是自然口语
// 不应过滤。左侧严格只伤"间接陈述句里提到名字"（「叫玄女」「他要找玄女」）这种边缘 case，
// 赚误触发率。
final class LocalWakeFallbackTests: XCTestCase {
    // MARK: 正例 —— 左侧 clean

    /// 单独 keyword：左 = string 起始
    func test_keywordAlone_isHit() {
        XCTAssertTrue(isWakeKeywordHit(transcript: "玄女", keyword: "玄女"))
    }

    /// 「玄女啊」：左 = string 起始（右"啊"是中文，不限）
    func test_keywordAtStart_chineseAfter_isHit() {
        XCTAssertTrue(isWakeKeywordHit(transcript: "玄女啊", keyword: "玄女"))
    }

    /// 「玄女你帮我看下」：左 = string 起始
    func test_keywordAtStart_longerSentence_isHit() {
        XCTAssertTrue(isWakeKeywordHit(transcript: "玄女你帮我看下", keyword: "玄女"))
    }

    /// 「贾维斯」：左 = string 起始
    func test_jiawesi_alone_isHit() {
        XCTAssertTrue(isWakeKeywordHit(transcript: "贾维斯", keyword: "贾维斯"))
    }

    /// 标点边界：「嗯，玄女」左侧是中文全角逗号（U+FF0C，不在 CJK 表意区），算 clean
    func test_keywordWithFullwidthCommaBoundary_isHit() {
        XCTAssertTrue(isWakeKeywordHit(transcript: "嗯，玄女，过来一下", keyword: "玄女"))
    }

    /// ASCII 空格边界：「hi 玄女 hello」
    func test_keywordWithAsciiBoundary_isHit() {
        XCTAssertTrue(isWakeKeywordHit(transcript: "hi 玄女 hello", keyword: "玄女"))
    }

    /// 第一次嵌入（左中文） false，但句中又出现一次干净的（左侧是中文逗号）→ 总体 true
    func test_multipleHits_takeFirstClean() {
        XCTAssertTrue(isWakeKeywordHit(transcript: "她穿玄女装，玄女，过来", keyword: "玄女"))
    }

    // MARK: 负例 —— 左侧不 clean / 不含 substring

    /// 「叫玄女」：左"叫"是中文，不算边界 → false（team-lead 修正后预期）
    func test_jiao_xuannv_isNotHit() {
        XCTAssertFalse(isWakeKeywordHit(transcript: "叫玄女", keyword: "玄女"))
    }

    /// 「叫玄女过来」：同上 false（team-lead 修正后预期）
    func test_jiao_xuannv_guolai_isNotHit() {
        XCTAssertFalse(isWakeKeywordHit(transcript: "叫玄女过来", keyword: "玄女"))
    }

    /// 单字「玄」不含完整 keyword
    func test_singleCharOnly_isNotHit() {
        XCTAssertFalse(isWakeKeywordHit(transcript: "玄", keyword: "玄女"))
    }

    /// 「他是个现代女孩」根本不含 substring「玄女」
    func test_homophoneOfXiandai_isNotHit() {
        XCTAssertFalse(isWakeKeywordHit(transcript: "他是个现代女孩", keyword: "玄女"))
    }

    /// 「鲁班学女工」根本不含 substring「玄女」
    func test_homophoneOfXuenv_isNotHit() {
        XCTAssertFalse(isWakeKeywordHit(transcript: "鲁班学女工", keyword: "玄女"))
    }

    /// 嵌入复合词「玄女装」：左中文 → false
    func test_keywordEmbeddedInLongerWord_isNotHit() {
        XCTAssertFalse(isWakeKeywordHit(transcript: "她穿了一身玄女装出门", keyword: "玄女"))
    }

    // MARK: 边界

    func test_emptyTranscript_isNotHit() {
        XCTAssertFalse(isWakeKeywordHit(transcript: "", keyword: "玄女"))
    }

    func test_emptyKeyword_isNotHit() {
        XCTAssertFalse(isWakeKeywordHit(transcript: "玄女", keyword: ""))
    }
}
