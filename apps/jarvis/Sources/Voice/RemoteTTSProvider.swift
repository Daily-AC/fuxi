import AVFoundation
import Foundation
import OSLog

/// 远端 TTS provider —— 调 home 上的 GPT-SoVITS API，拿 wav，AVAudioPlayer 播。
///
/// 取代场景：用户希望换派蒙 / 钟离 / 胡桃等角色音色——AVSpeechSynthesizer 系统音
/// 色做不了，要靠 GPT-SoVITS / Bert-VITS2 这种 voice cloning 服务。
///
/// 协议（最小版）：
/// - `POST <baseURL>` body `{"text": "..."}` Header `Authorization: Bearer <token>`
/// - 200 OK → wav bytes（content-type audio/wav）
/// - 4xx/5xx → 走 fallback（调用方决定退回 system TTS 还是吞）
///
/// 取消语义：连续两条语音时，AppState 会先 stop() 抢占——AVAudioPlayer 立即 stop，
/// 进行中的 HTTP 请求 cancel（URLSession dataTask），onFinish 回调不发。
@MainActor
final class RemoteTTSProvider: NSObject, AVAudioPlayerDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "tts.remote")
    private let session: URLSession
    private var player: AVAudioPlayer?
    private var currentTask: URLSessionDataTask?
    private var onFinish: (() -> Void)?

    override init() {
        // 长 timeout——GPT-SoVITS V4 在 5090 上 < 0.5s，但首次推理冷启动 / 长句最多 3-5s。
        // 给 15s 兜底，超出说明 server 出问题，让用户感知到（不要无限等）。
        let cfg = URLSessionConfiguration.default
        cfg.timeoutIntervalForRequest = 15
        cfg.timeoutIntervalForResource = 30
        self.session = URLSession(configuration: cfg)
        super.init()
    }

    /// 调远端 TTS 拿 wav 播放。失败时 fallback 调用方在 completion 看到 `success=false`。
    func speak(
        _ text: String,
        baseURL: String,
        bearerToken: String,
        completion: @escaping (Bool) -> Void
    ) {
        // 抢占：先 stop 当前播放 + cancel 进行中请求。
        stop()
        onFinish = { completion(true) }

        guard let url = URL(string: baseURL) else {
            logger.error("remote tts url 不合法: \(baseURL, privacy: .public)")
            completion(false)
            return
        }

        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if !bearerToken.isEmpty {
            req.setValue("Bearer \(bearerToken)", forHTTPHeaderField: "Authorization")
        }
        let body = ["text": text]
        req.httpBody = try? JSONSerialization.data(withJSONObject: body)

        let task = session.dataTask(with: req) { [weak self] data, resp, err in
            Task { @MainActor in
                guard let self = self else { return }
                if let err = err {
                    let nserr = err as NSError
                    if nserr.code == NSURLErrorCancelled {
                        // 抢占造成的 cancel 不算 fail，调用方已经发起新请求。
                        return
                    }
                    self.logger.error("remote tts 请求失败: \(err.localizedDescription, privacy: .public)")
                    completion(false)
                    return
                }
                guard let http = resp as? HTTPURLResponse, http.statusCode == 200,
                      let data = data, !data.isEmpty else {
                    let code = (resp as? HTTPURLResponse)?.statusCode ?? -1
                    self.logger.error("remote tts http \(code), bytes=\(data?.count ?? 0)")
                    completion(false)
                    return
                }
                self.playWav(data, completion: completion)
            }
        }
        currentTask = task
        task.resume()
    }

    private func playWav(_ data: Data, completion: @escaping (Bool) -> Void) {
        do {
            let p = try AVAudioPlayer(data: data)
            p.delegate = self
            p.prepareToPlay()
            self.onFinish = { completion(true) }
            self.player = p
            p.play()
            logger.debug("remote tts 播放 \(data.count) bytes wav")
        } catch {
            logger.error("AVAudioPlayer init 失败: \(error.localizedDescription, privacy: .public)")
            completion(false)
        }
    }

    func stop() {
        currentTask?.cancel()
        currentTask = nil
        player?.stop()
        player = nil
        onFinish = nil
    }

    nonisolated func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        Task { @MainActor in
            let cb = self.onFinish
            self.onFinish = nil
            self.player = nil
            cb?()
        }
    }

    nonisolated func audioPlayerDecodeErrorDidOccur(_ player: AVAudioPlayer, error: Error?) {
        Task { @MainActor in
            self.logger.error("AVAudioPlayer decode error: \(error?.localizedDescription ?? "nil", privacy: .public)")
            let cb = self.onFinish
            self.onFinish = nil
            self.player = nil
            cb?()
        }
    }
}
