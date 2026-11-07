import ExpoModulesCore
import MonocodeLinkCore
import Network

public class MonocodeLinkModule: Module {
  public func definition() -> ModuleDefinition {
    Name("MonocodeLink")
    OnCreate { LinkNetworkWatch.shared.start() }
    Constant("nativeTranscriptVersion") { 1 }
    Constant("typingPostureVersion") { 1 }
    View(NativeTranscriptView.self) {
      Events("onStatus", "onAction", "onScrollState", "onDraft")
      Prop("configuration") { (view: NativeTranscriptView, value: String) in
        view.setConfiguration(value)
      }
      Prop("pending") { (view: NativeTranscriptView, value: String) in view.setPending(value) }
      Prop("appearance") { (view: NativeTranscriptView, value: String) in view.setAppearance(value)
      }
      Prop("active") { (view: NativeTranscriptView, value: Bool) in view.setActive(value) }
      Prop("topInset") { (view: NativeTranscriptView, value: Double) in view.setInsets(top: value) }
      Prop("bottomInset") { (view: NativeTranscriptView, value: Double) in
        view.setInsets(bottom: value)
      }
      Prop("composerExtra") { (view: NativeTranscriptView, value: Double) in
        view.setInsets(extra: value)
      }
      Prop("keyboardOffset") { (view: NativeTranscriptView, value: Double) in
        view.setInsets(keyboardOffset: value)
      }
      OnViewDidUpdateProps { (view: NativeTranscriptView) in view.updateProps() }
      AsyncFunction("scrollToEnd") { (view: NativeTranscriptView) in view.jumpToEnd() }
      AsyncFunction("selectText") { (view: NativeTranscriptView, text: String) in
        view.selectText(text)
      }
      AsyncFunction("draftChrome") { (view: NativeTranscriptView, id: String, chrome: String) in
        view.setDraftChrome(id, chrome)
      }
      AsyncFunction("draftLoad") {
        (view: NativeTranscriptView, id: String, html: String, generation: Int) in
        view.loadDraft(id, html: html, generation: generation)
      }
      AsyncFunction("draftScript") { (view: NativeTranscriptView, id: String, script: String) in
        view.runDraftScript(id, script)
      }
      AsyncFunction("draftPicture") {
        (view: NativeTranscriptView, id: String, uri: String?, ratio: Double?) in
        view.setDraftPicture(id, uri, ratio: ratio)
      }
      AsyncFunction("scrollToDraft") { (view: NativeTranscriptView, draft: String) in
        view.scrollToDraft(draft)
      }
    }
    View(TypingPostureView.self) {
      Events("onEnterTypingPosture")
      Prop("enabled") { (view: TypingPostureView, value: Bool) in view.enabled = value }
      OnViewDidUpdateProps { (view: TypingPostureView) in view.updatePosture() }
    }
    AsyncFunction("hideScrollEdgeEffects") { (testID: String) -> Bool in
      MainActor.assumeIsolated { ScrollEdgeEffects.hide(testID: testID) }
    }.runOnQueue(.main)
    AsyncFunction("execute") { (input: String) -> String in
      MonocodeLinkCall.execute(input)
    }.runOnQueue(.global(qos: .userInitiated))
  }
}

/// The link's one entry point, for native callers such as the Live
/// Activity's approval buttons. Blocks until the desktop answers.
public enum MonocodeLinkCall {
  public static func execute(_ input: String) -> String {
    input.withCString { pointer in
      guard let output = monocode_link_call(pointer) else {
        return "{\"ok\":false,\"error\":\"Native connection failed\"}"
      }
      defer { monocode_link_free(output) }
      return String(cString: output)
    }
  }
}

/// Tells the link when the phone moves to another network, so it reconnects at
/// once. Pings are far enough apart that it would take much longer to notice.
final class LinkNetworkWatch {
  static let shared = LinkNetworkWatch()
  private let monitor = NWPathMonitor()
  private let queue = DispatchQueue(label: "monocode.link.network", qos: .utility)
  private var started = false
  private var route: String?
  private var settling: DispatchWorkItem?

  func start() {
    queue.async { [self] in
      guard !started else { return }
      started = true
      monitor.pathUpdateHandler = { [weak self] path in self?.update(path) }
      monitor.start(queue: queue)
    }
  }

  private func update(_ path: NWPath) {
    guard path.status == .satisfied else { return }
    let next = Self.route(path)
    guard let known = route else {
      route = next
      return
    }
    settling?.cancel()
    settling = nil
    guard next != known else { return }
    // A handover reports several paths in a row; act on where it comes to rest.
    let work = DispatchWorkItem { [weak self] in
      self?.route = next
      _ = MonocodeLinkCall.execute("{\"operation\":\"networkChanged\"}")
    }
    settling = work
    queue.asyncAfter(deadline: .now() + 1, execute: work)
  }

  private static func route(_ path: NWPath) -> String {
    let interface = path.availableInterfaces.first?.name ?? ""
    return ([interface] + path.gateways.map { "\($0)" }).joined(separator: " ")
  }
}
