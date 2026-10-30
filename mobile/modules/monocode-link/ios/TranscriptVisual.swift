import UIKit

/// An agent's interactive visual in the phone's transcript. The program runs
/// in the native link (the desktop's runtime); this lays out its component
/// tree for the phone's width and draws it with native controls. A control's
/// change goes back as a transcript command, and the row comes back with the
/// new tree.

/// Colors and type, from the transcript's theme and the desktop's scale.
struct VisualTheme {
  let style: TranscriptStyle
  var ink: UIColor { UIColor.transcriptHex(style.content) }
  var dark: Bool {
    var white: CGFloat = 0
    ink.getWhite(&white, alpha: nil)
    return white > 0.5
  }
  func color(_ name: Any?) -> UIColor {
    let pick = { (dark: UInt32, light: UInt32) -> UIColor in
      let value = self.dark ? dark : light
      return UIColor(
        red: CGFloat((value >> 16) & 255) / 255, green: CGFloat((value >> 8) & 255) / 255,
        blue: CGFloat(value & 255) / 255, alpha: 1)
    }
    switch name as? String ?? "ink" {
    case "muted": return ink.withAlphaComponent(0.6)
    case "faint": return ink.withAlphaComponent(0.2)
    case "accent", "series1": return UIColor.transcriptHex(style.accent)
    case "positive", "series2": return pick(0x4fd57f, 0x0a924b)
    case "negative": return pick(0xfc5855, 0xd01d21)
    case "warning", "series4": return pick(0xfea247, 0xd96700)
    case "series3": return pick(0xb77df8, 0x863ad2)
    case "series5": return pick(0xfa78b2, 0xd12c80)
    case "series6": return pick(0x3faff3, 0x0077d7)
    default: return ink
    }
  }
  func font(_ size: Any?, weight: UIFont.Weight? = nil) -> UIFont {
    let (points, heavy): (CGFloat, Bool) =
      switch size as? String ?? "body" {
      case "caption": (11, false)
      case "small": (12, false)
      case "subtitle": (17, true)
      case "title": (22, true)
      case "hero": (34, true)
      default: (14, false)
      }
    return .systemFont(ofSize: points * style.scale, weight: weight ?? (heavy ? .semibold : .regular))
  }
  func measure(_ text: String, font: UIFont, width: CGFloat) -> CGSize {
    let bounds = (text as NSString).boundingRect(
      with: CGSize(width: max(1, width), height: .greatestFiniteMagnitude),
      options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: font], context: nil)
    return CGSize(width: ceil(bounds.width), height: ceil(bounds.height))
  }
}

/// One thing to draw or one control, at its place in the visual. `path` is
/// the node's position in the tree, so a control keeps its view (and a drag
/// in progress) across new trees.
struct VisualPiece {
  let path: String
  let kind: String
  let node: [String: Any]
  let rect: CGRect
}

/// Built on the transcript worker: measuring only, no views.
struct VisualLayout {
  private(set) var pieces: [VisualPiece] = []
  private(set) var height: CGFloat = 0
  let theme: VisualTheme
  /// Controls narrower than this wrap onto the next line on a phone.
  private static let minimumCell: CGFloat = 132

  init(tree: [String: Any], width: CGFloat, theme: VisualTheme) {
    self.theme = theme
    height = place(tree, path: "0", x: 0, y: 0, width: width)
  }

  private mutating func place(
    _ node: [String: Any], path: String, x: CGFloat, y: CGFloat, width: CGFloat
  ) -> CGFloat {
    let kind = node["kind"] as? String ?? ""
    let children = node["children"] as? [[String: Any]] ?? []
    let gap = node["gap"] as? CGFloat ?? 12
    func piece(_ height: CGFloat) -> CGFloat {
      pieces.append(
        VisualPiece(path: path, kind: kind, node: node, rect: CGRect(x: x, y: y, width: width, height: height)))
      return height
    }
    let s = theme.style.scale
    switch kind {
    case "column":
      return stack(children, path: path, x: x, y: y, width: width, gap: gap)
    case "card":
      let index = pieces.count
      let inner = stack(children, path: path, x: x + 14, y: y + 14, width: width - 28, gap: gap)
      pieces.insert(
        VisualPiece(path: path, kind: "card", node: node, rect: CGRect(x: x, y: y, width: width, height: inner + 28)),
        at: index)
      return inner + 28
    case "row", "grid":
      // A row of controls or stats becomes a grid that wraps on a phone.
      let declared = kind == "grid" ? max(1, node["columns"] as? Int ?? 2) : max(1, children.count)
      let fits = max(1, Int((width + gap) / (Self.minimumCell + gap)))
      let columns = min(declared, fits)
      let cell = (width - gap * CGFloat(columns - 1)) / CGFloat(columns)
      var top = y
      var start = 0
      while start < children.count {
        let line = children[start..<min(children.count, start + columns)]
        var tallest: CGFloat = 0
        for (offset, child) in line.enumerated() {
          let h = place(
            child, path: "\(path).\(start + offset)", x: x + CGFloat(offset) * (cell + gap), y: top, width: cell)
          tallest = max(tallest, h)
        }
        // Cards side by side line up at the bottom too.
        let paths = Set(line.indices.map { "\(path).\($0)" })
        for index in pieces.indices where pieces[index].kind == "card" && paths.contains(pieces[index].path) {
          let card = pieces[index]
          pieces[index] = VisualPiece(
            path: card.path, kind: card.kind, node: card.node,
            rect: CGRect(x: card.rect.minX, y: card.rect.minY, width: card.rect.width, height: tallest))
        }
        top += tallest + gap
        start += columns
      }
      return max(0, top - y - (children.isEmpty ? 0 : gap))
    case "text":
      let font = theme.font(node["size"])
      return piece(theme.measure(node["text"] as? String ?? "", font: font, width: width).height)
    case "stat":
      var h = 16 * s + 2 + theme.font("title").lineHeight
      if node["delta"] is String { h += 2 + 16 * s }
      if node["detail"] is String { h += 2 + 16 * s }
      if (node["trend"] as? [Double] ?? []).count > 1 { h += 6 + 28 }
      return piece(ceil(h))
    case "table":
      let rows = (node["rows"] as? [[Any]] ?? []).count
      return piece(CGFloat(rows + 1) * 34 * s)
    case "chart":
      let series = (node["series"] as? [[String: Any]] ?? []).count
      let plot = min(280, max(150, node["height"] as? CGFloat ?? 220))
      return piece(plot + 20 + (series > 1 ? 24 : 0))
    case "gauge":
      return piece(min(width, 220) * 0.55 + 40 * s)
    case "donut":
      let slices = (node["slices"] as? [Any] ?? []).count
      let side = width >= 300
      return piece(side ? max(140, CGFloat(slices) * 22 * s) : 140 + 8 + CGFloat(slices) * 22 * s)
    case "heatmap":
      let rows = (node["values"] as? [[Double]] ?? []).count
      let columns = max(1, (node["values"] as? [[Double]] ?? []).map(\.count).max() ?? 1)
      let cell = min(28, (width - 64) / CGFloat(columns))
      return piece(18 * s + CGFloat(rows) * cell)
    case "progress":
      return piece(18 * s + 6 + 6 + (node["detail"] is String ? 4 + 16 * s : 0))
    case "button", "toggle":
      return piece(44)
    case "input", "select":
      return piece(18 * s + 4 + 44)
    case "slider":
      return piece(18 * s + 36)
    case "tabs":
      return piece(36)
    case "canvas", "scene":
      let w = max(1, node["width"] as? CGFloat ?? 1)
      let h = max(1, node["height"] as? CGFloat ?? 1)
      return piece(min(kind == "scene" ? 340 : 520, width * h / w))
    default:
      return 0
    }
  }

  private mutating func stack(
    _ children: [[String: Any]], path: String, x: CGFloat, y: CGFloat, width: CGFloat, gap: CGFloat
  ) -> CGFloat {
    var top = y
    for (index, child) in children.enumerated() {
      let h = place(child, path: "\(path).\(index)", x: x, y: top, width: width)
      top += h + (index < children.count - 1 && h > 0 ? gap : 0)
    }
    return top - y
  }
}

/// Draws one non-interactive piece.
private final class VisualDrawing: UIView {
  var piece: VisualPiece?
  var theme: VisualTheme?

  override init(frame: CGRect) {
    super.init(frame: frame)
    isOpaque = false
    backgroundColor = .clear
    contentMode = .redraw
  }
  required init?(coder: NSCoder) { fatalError() }

  private func text(
    _ value: String, at point: CGPoint, font: UIFont, color: UIColor, width: CGFloat? = nil,
    align: NSTextAlignment = .left
  ) {
    let paragraph = NSMutableParagraphStyle()
    paragraph.alignment = align
    paragraph.lineBreakMode = .byTruncatingTail
    let attributes: [NSAttributedString.Key: Any] = [
      .font: font, .foregroundColor: color, .paragraphStyle: paragraph,
    ]
    if let width {
      (value as NSString).draw(
        with: CGRect(x: point.x, y: point.y, width: width, height: font.lineHeight * 1.2),
        options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine], attributes: attributes, context: nil)
    } else {
      (value as NSString).draw(at: point, withAttributes: attributes)
    }
  }

  override func draw(_ rect: CGRect) {
    guard let piece, let theme, let context = UIGraphicsGetCurrentContext() else { return }
    let node = piece.node
    let size = bounds.size
    let s = theme.style.scale
    let small = UIFont.systemFont(ofSize: 12 * s)
    let muted = theme.color("muted")
    switch piece.kind {
    case "card":
      let path = UIBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), cornerRadius: 12)
      theme.ink.withAlphaComponent(0.04).setFill()
      path.fill()
      theme.ink.withAlphaComponent(0.1).setStroke()
      path.lineWidth = 1 / max(1, theme.style.pixelRatio)
      path.stroke()
    case "text":
      let font = theme.font(node["size"])
      ((node["text"] as? String ?? "") as NSString).draw(
        with: bounds, options: [.usesLineFragmentOrigin, .usesFontLeading],
        attributes: [.font: font, .foregroundColor: theme.color(node["color"])], context: nil)
    case "stat":
      var y: CGFloat = 0
      text(node["label"] as? String ?? "", at: CGPoint(x: 0, y: y), font: small, color: muted, width: size.width)
      y += 16 * s + 2
      let valueFont = theme.font("title")
      text(
        node["value"] as? String ?? "", at: CGPoint(x: 0, y: y), font: valueFont,
        color: theme.color(node["color"]), width: size.width)
      y += valueFont.lineHeight
      if let delta = node["delta"] as? String {
        let falling = delta.hasPrefix("-") || delta.hasPrefix("−")
        let good = falling == (node["lower_is_better"] as? Bool == true)
        let shown = (falling ? "↓ " : "↑ ") + delta.trimmingCharacters(in: CharacterSet(charactersIn: "+-−"))
        text(shown, at: CGPoint(x: 0, y: y + 2), font: small, color: theme.color(good ? "positive" : "negative"), width: size.width)
        y += 2 + 16 * s
      }
      if let detail = node["detail"] as? String {
        text(detail, at: CGPoint(x: 0, y: y + 2), font: small, color: muted, width: size.width)
        y += 2 + 16 * s
      }
      let trend = node["trend"] as? [Double] ?? []
      if trend.count > 1 {
        spark(trend, in: CGRect(x: 0, y: y + 6, width: size.width, height: 28), color: theme.color(node["color"]))
      }
    case "table":
      let columns = node["columns"] as? [String] ?? []
      let rows = node["rows"] as? [[String]] ?? []
      let row = 34 * s
      let cell = size.width / CGFloat(max(1, columns.count))
      let header = UIFont.systemFont(ofSize: 12 * s, weight: .semibold)
      let body = UIFont.systemFont(ofSize: 13 * s)
      for (i, column) in columns.enumerated() {
        text(column, at: CGPoint(x: CGFloat(i) * cell + 8, y: 9 * s), font: header, color: muted, width: cell - 16)
      }
      for (r, values) in rows.enumerated() {
        let y = CGFloat(r + 1) * row
        context.setFillColor(theme.ink.withAlphaComponent(0.08).cgColor)
        context.fill(CGRect(x: 0, y: y, width: size.width, height: 1 / max(1, theme.style.pixelRatio)))
        for (i, value) in values.prefix(columns.count).enumerated() {
          text(value, at: CGPoint(x: CGFloat(i) * cell + 8, y: y + 9 * s), font: body, color: theme.ink, width: cell - 16)
        }
      }
    case "chart":
      chart(node, theme: theme, context: context)
    case "gauge":
      let minimum = node["min"] as? Double ?? 0
      let maximum = node["max"] as? Double ?? 1
      let value = node["value"] as? Double ?? 0
      let t = CGFloat(max(0, min(1, (value - minimum) / max(0.000001, maximum - minimum))))
      let radius = min(size.width, 220) / 2 - 8
      let center = CGPoint(x: size.width / 2, y: radius + 8)
      let track = UIBezierPath(arcCenter: center, radius: radius, startAngle: .pi, endAngle: 2 * .pi, clockwise: true)
      track.lineWidth = 12
      track.lineCapStyle = .round
      theme.ink.withAlphaComponent(0.1).setStroke()
      track.stroke()
      let fill = UIBezierPath(
        arcCenter: center, radius: radius, startAngle: .pi, endAngle: .pi + .pi * t, clockwise: true)
      fill.lineWidth = 12
      fill.lineCapStyle = .round
      theme.color(node["color"] ?? "accent").setStroke()
      fill.stroke()
      let unit = node["unit"] as? String ?? ""
      let shown = Self.number(value) + (unit.isEmpty || unit == "%" ? unit : " " + unit)
      let big = theme.font("title")
      text(shown, at: CGPoint(x: 0, y: center.y - big.lineHeight), font: big, color: theme.ink, width: size.width, align: .center)
      text(node["label"] as? String ?? "", at: CGPoint(x: 0, y: center.y + 8), font: small, color: muted, width: size.width, align: .center)
    case "donut":
      let slices = node["slices"] as? [[String: Any]] ?? []
      let total = max(0.000001, slices.reduce(0) { $0 + ($1["value"] as? Double ?? 0) })
      let side = size.width >= 300
      let ring = CGRect(x: side ? 0 : (size.width - 132) / 2, y: 4, width: 132, height: 132)
      let center = CGPoint(x: ring.midX, y: ring.midY)
      var angle = -CGFloat.pi / 2
      for slice in slices {
        let sweep = CGFloat((slice["value"] as? Double ?? 0) / total) * 2 * .pi
        let arc = UIBezierPath(arcCenter: center, radius: 54, startAngle: angle, endAngle: angle + sweep, clockwise: true)
        arc.lineWidth = 20
        theme.color(slice["color"] ?? "accent").setStroke()
        arc.stroke()
        angle += sweep
      }
      if let value = node["value"] as? String {
        let font = UIFont.systemFont(ofSize: 17 * s, weight: .semibold)
        text(value, at: CGPoint(x: ring.minX, y: center.y - font.lineHeight / 2 - (node["label"] is String ? 7 : 0)), font: font, color: theme.ink, width: ring.width, align: .center)
      }
      if let label = node["label"] as? String {
        text(label, at: CGPoint(x: ring.minX, y: center.y + 4), font: small, color: muted, width: ring.width, align: .center)
      }
      let legendX = side ? ring.maxX + 20 : 0
      var y = side ? max(4, (size.height - CGFloat(slices.count) * 22 * s) / 2) : ring.maxY + 8
      for slice in slices {
        context.setFillColor(theme.color(slice["color"] ?? "accent").cgColor)
        context.fillEllipse(in: CGRect(x: legendX, y: y + 5 * s, width: 8, height: 8))
        let value = Self.number(slice["value"] as? Double ?? 0)
        text("\(slice["label"] as? String ?? "")  \(value)", at: CGPoint(x: legendX + 16, y: y), font: small, color: theme.ink, width: size.width - legendX - 16)
        y += 22 * s
      }
    case "heatmap":
      let values = node["values"] as? [[Double]] ?? []
      let rows = node["rows"] as? [String] ?? []
      let columns = node["columns"] as? [String] ?? []
      let count = max(1, values.map(\.count).max() ?? 1)
      let cell = min(28, (size.width - 64) / CGFloat(count))
      let flat = values.flatMap { $0 }
      let low = flat.min() ?? 0
      let high = flat.max() ?? 1
      let tint = theme.color(node["color"] ?? "accent")
      let caption = UIFont.systemFont(ofSize: 10 * s)
      for (i, column) in columns.prefix(count).enumerated() where i % max(1, Int(40 / cell)) == 0 {
        text(column, at: CGPoint(x: 64 + CGFloat(i) * cell, y: 0), font: caption, color: muted)
      }
      for (r, row) in values.enumerated() {
        let y = 18 * s + CGFloat(r) * cell
        if r < rows.count { text(rows[r], at: CGPoint(x: 0, y: y + (cell - caption.lineHeight) / 2), font: caption, color: muted, width: 60) }
        for (c, value) in row.enumerated() {
          let t = high > low ? CGFloat((value - low) / (high - low)) : 1
          context.setFillColor(tint.withAlphaComponent(0.08 + 0.92 * t).cgColor)
          context.fill(CGRect(x: 64 + CGFloat(c) * cell + 1, y: y + 1, width: cell - 2, height: cell - 2))
        }
      }
    case "progress":
      let maximum = max(0.000001, node["max"] as? Double ?? 1)
      let t = CGFloat(max(0, min(1, (node["value"] as? Double ?? 0) / maximum)))
      text(node["label"] as? String ?? "", at: .zero, font: small, color: theme.ink, width: size.width - 60)
      text(Self.number((node["value"] as? Double ?? 0) / maximum * 100) + "%", at: CGPoint(x: size.width - 60, y: 0), font: small, color: muted, width: 60, align: .right)
      let bar = CGRect(x: 0, y: 18 * s + 6, width: size.width, height: 6)
      context.setFillColor(theme.ink.withAlphaComponent(0.1).cgColor)
      UIBezierPath(roundedRect: bar, cornerRadius: 3).fill()
      theme.color(node["color"] ?? "accent").setFill()
      UIBezierPath(roundedRect: CGRect(x: 0, y: bar.minY, width: bar.width * t, height: 6), cornerRadius: 3).fill()
      if let detail = node["detail"] as? String {
        text(detail, at: CGPoint(x: 0, y: bar.maxY + 4), font: small, color: muted, width: size.width)
      }
    case "canvas":
      let w = max(1, node["width"] as? CGFloat ?? 1)
      let scale = size.width / w
      context.scaleBy(x: scale, y: scale)
      for shape in node["shapes"] as? [[String: Any]] ?? [] {
        let color = theme.color(shape["color"])
        let f = { (key: String) in shape[key] as? CGFloat ?? 0 }
        switch shape["kind"] as? String {
        case "rect":
          color.setFill()
          context.fill(CGRect(x: f("x"), y: f("y"), width: f("width"), height: f("height")))
        case "circle":
          color.setFill()
          context.fillEllipse(in: CGRect(x: f("x") - f("radius"), y: f("y") - f("radius"), width: f("radius") * 2, height: f("radius") * 2))
        case "line":
          color.setStroke()
          context.setLineWidth(max(0.5, f("width")))
          context.move(to: CGPoint(x: f("x1"), y: f("y1")))
          context.addLine(to: CGPoint(x: f("x2"), y: f("y2")))
          context.strokePath()
        case "polyline":
          let points = (shape["points"] as? [[CGFloat]] ?? []).filter { $0.count == 2 }
          guard let first = points.first else { continue }
          color.setStroke()
          context.setLineWidth(max(0.5, f("width")))
          context.setLineJoin(.round)
          context.move(to: CGPoint(x: first[0], y: first[1]))
          for point in points.dropFirst() { context.addLine(to: CGPoint(x: point[0], y: point[1])) }
          context.strokePath()
        case "label":
          // Labels stay readable however far the drawing is scaled down.
          let font = UIFont.systemFont(ofSize: max(12, 12 / scale) * s)
          text(shape["text"] as? String ?? "", at: CGPoint(x: f("x"), y: f("y") - font.lineHeight / 2), font: font, color: color)
        default: break
        }
      }
    default:
      break
    }
  }

  private func spark(_ values: [Double], in rect: CGRect, color: UIColor) {
    guard let low = values.min(), let high = values.max() else { return }
    let path = UIBezierPath()
    for (i, value) in values.enumerated() {
      let point = CGPoint(
        x: rect.minX + rect.width * CGFloat(i) / CGFloat(values.count - 1),
        y: rect.maxY - rect.height * CGFloat(high > low ? (value - low) / (high - low) : 0.5))
      i == 0 ? path.move(to: point) : path.addLine(to: point)
    }
    color.setStroke()
    path.lineWidth = 1.5
    path.lineJoinStyle = .round
    path.stroke()
  }

  private func chart(_ node: [String: Any], theme: VisualTheme, context: CGContext) {
    let series = node["series"] as? [[String: Any]] ?? []
    let labels = node["labels"] as? [String] ?? []
    let type = node["chart_type"] as? String ?? "bar"
    let s = theme.style.scale
    let caption = UIFont.systemFont(ofSize: 10 * s)
    var top: CGFloat = 0
    if series.count > 1 {
      var x: CGFloat = 0
      for item in series {
        let label = item["label"] as? String ?? ""
        context.setFillColor(theme.color(item["color"] ?? "accent").cgColor)
        context.fillEllipse(in: CGRect(x: x, y: 5, width: 8, height: 8))
        text(label, at: CGPoint(x: x + 12, y: 0), font: UIFont.systemFont(ofSize: 12 * s), color: theme.ink)
        x += 24 + theme.measure(label, font: .systemFont(ofSize: 12 * s), width: 400).width
      }
      top = 24
    }
    let values = series.flatMap { $0["values"] as? [Double] ?? [] }
    let high = max(values.max() ?? 1, type == "bar" ? 0 : -Double.infinity)
    let low = min(values.min() ?? 0, 0)
    let plot = CGRect(x: 32, y: top + 4, width: bounds.width - 32, height: bounds.height - top - 24)
    let y = { (value: Double) in plot.maxY - plot.height * CGFloat((value - low) / max(0.000001, high - low)) }
    // Three guide lines with their values, as on the desktop.
    for step in 0...2 {
      let value = low + (high - low) * Double(step) / 2
      context.setFillColor(theme.ink.withAlphaComponent(0.08).cgColor)
      context.fill(CGRect(x: plot.minX, y: y(value), width: plot.width, height: 1 / max(1, theme.style.pixelRatio)))
      text(Self.number(value), at: CGPoint(x: 0, y: y(value) - caption.lineHeight / 2), font: caption, color: theme.color("muted"), width: 28, align: .right)
    }
    let count = max(1, series.map { ($0["values"] as? [Double] ?? []).count }.max() ?? 1)
    let slot = plot.width / CGFloat(count)
    // Only as many labels as fit at phone width.
    let every = max(1, Int(ceil(40 / slot)))
    for (i, label) in labels.prefix(count).enumerated() where i % every == 0 {
      text(label, at: CGPoint(x: plot.minX + CGFloat(i) * slot, y: plot.maxY + 6), font: caption, color: theme.color("muted"), width: slot * CGFloat(every), align: .left)
    }
    for (index, item) in series.enumerated() {
      let data = item["values"] as? [Double] ?? []
      let color = theme.color(item["color"] ?? "series\(index + 1)")
      if type == "bar" {
        let width = max(1, slot * 0.7 / CGFloat(series.count))
        color.setFill()
        for (i, value) in data.enumerated() {
          let x = plot.minX + CGFloat(i) * slot + slot * 0.15 + CGFloat(index) * width
          let top = min(y(value), y(0))
          UIBezierPath(roundedRect: CGRect(x: x, y: top, width: width - 1, height: max(1, abs(y(value) - y(0)))), cornerRadius: min(3, width / 3)).fill()
        }
      } else {
        let path = UIBezierPath()
        for (i, value) in data.enumerated() {
          let point = CGPoint(x: plot.minX + CGFloat(i) * slot + slot / 2, y: y(value))
          i == 0 ? path.move(to: point) : path.addLine(to: point)
        }
        if type == "area", data.count > 1 {
          let fill = path.copy() as! UIBezierPath
          fill.addLine(to: CGPoint(x: plot.minX + CGFloat(data.count - 1) * slot + slot / 2, y: y(low)))
          fill.addLine(to: CGPoint(x: plot.minX + slot / 2, y: y(low)))
          fill.close()
          color.withAlphaComponent(0.18).setFill()
          fill.fill()
        }
        color.setStroke()
        path.lineWidth = 2
        path.lineJoinStyle = .round
        path.stroke()
      }
    }
  }

  static func number(_ value: Double) -> String {
    if abs(value) >= 1000 || value == value.rounded() {
      return value.formatted(.number.precision(.fractionLength(0)))
    }
    return value.formatted(.number.precision(.fractionLength(0...2)))
  }
}

/// A 3D scene: projected by the native link with the desktop's maths, so a
/// horizontal drag orbits it and a pinch zooms. Vertical drags scroll the
/// transcript.
private final class VisualScene: UIView, UIGestureRecognizerDelegate {
  var node: [String: Any] = [:] {
    didSet {
      if (oldValue as NSDictionary) != (node as NSDictionary) { project() }
    }
  }
  var theme: VisualTheme?
  private var orbit = (yaw: CGFloat(0), pitch: CGFloat(0), zoom: CGFloat(1))
  private var start = (yaw: CGFloat(0), pitch: CGFloat(0), zoom: CGFloat(1))
  private var surfaces: [[String: Any]] = []
  private let queue = DispatchQueue(label: "monocode.visual.scene")
  private var generation = 0

  override init(frame: CGRect) {
    super.init(frame: frame)
    isOpaque = false
    backgroundColor = .clear
    contentMode = .redraw
    let pan = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))
    pan.delegate = self
    addGestureRecognizer(pan)
    addGestureRecognizer(UIPinchGestureRecognizer(target: self, action: #selector(pinched(_:))))
    isAccessibilityElement = true
    accessibilityLabel = "3D scene"
    accessibilityHint = "Drag sideways to turn it, pinch to zoom."
  }
  required init?(coder: NSCoder) { fatalError() }

  override func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
    guard let pan = gesture as? UIPanGestureRecognizer else { return true }
    let velocity = pan.velocity(in: self)
    return abs(velocity.x) > abs(velocity.y)
  }
  @objc private func panned(_ gesture: UIPanGestureRecognizer) {
    if gesture.state == .began { start = orbit }
    let moved = gesture.translation(in: self)
    orbit.yaw = start.yaw + moved.x / 120
    orbit.pitch = max(-1.4, min(1.4, start.pitch + moved.y / 160))
    project()
  }
  @objc private func pinched(_ gesture: UIPinchGestureRecognizer) {
    if gesture.state == .began { start = orbit }
    orbit.zoom = max(0.3, min(4, start.zoom / max(0.01, gesture.scale)))
    project()
  }

  /// Projects off the main thread; only the latest orbit is drawn.
  private func project() {
    generation += 1
    let current = generation
    let request: [String: Any] = [
      "operation": "visualScene", "scene": node, "yaw": orbit.yaw, "pitch": orbit.pitch,
      "zoom": orbit.zoom,
    ]
    queue.async { [weak self] in
      let result = (try? TranscriptNative.call(request)) as? [String: Any]
      DispatchQueue.main.async {
        guard let self, current == self.generation else { return }
        self.surfaces = result?["surfaces"] as? [[String: Any]] ?? []
        self.setNeedsDisplay()
      }
    }
  }

  override func draw(_ rect: CGRect) {
    guard let theme, let context = UIGraphicsGetCurrentContext() else { return }
    let w = max(1, node["width"] as? CGFloat ?? 1)
    let h = max(1, node["height"] as? CGFloat ?? 1)
    let scale = min(bounds.width / w, bounds.height / h)
    context.translateBy(x: (bounds.width - w * scale) / 2, y: (bounds.height - h * scale) / 2)
    context.scaleBy(x: scale, y: scale)
    context.setLineJoin(.round)
    for surface in surfaces {
      let points = (surface["points"] as? [[CGFloat]] ?? []).filter { $0.count == 2 }
      guard let first = points.first else { continue }
      context.move(to: CGPoint(x: first[0], y: first[1]))
      for point in points.dropFirst() { context.addLine(to: CGPoint(x: point[0], y: point[1])) }
      let color = theme.color(surface["color"])
      if let width = surface["width"] as? CGFloat {
        context.setStrokeColor(color.cgColor)
        context.setLineWidth(max(0.5, width) / scale)
        context.strokePath()
      } else {
        context.closePath()
        let light = surface["light"] as? CGFloat ?? 1
        var (r, g, b, a): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
        color.getRed(&r, green: &g, blue: &b, alpha: &a)
        let shaded = UIColor(red: r * light, green: g * light, blue: b * light, alpha: a)
        context.setFillColor(shaded.cgColor)
        context.setStrokeColor(shaded.cgColor)
        context.setLineWidth(0.5 / scale)
        context.drawPath(using: .fillStroke)
      }
    }
  }
}

/// A labelled control: the label line above, the control below or beside it.
private final class VisualControl: UIView {
  let label = UILabel()
  let value = UILabel()
  var control: UIView?
  var send: ((Any) -> Void)?
  private var lastSent = Date.distantPast
  private var pending: Any?

  override init(frame: CGRect) {
    super.init(frame: frame)
    for text in [label, value] {
      text.numberOfLines = 1
      addSubview(text)
    }
    value.textAlignment = .right
  }
  required init?(coder: NSCoder) { fatalError() }

  func configure(_ piece: VisualPiece, theme: VisualTheme) {
    let node = piece.node
    let s = theme.style.scale
    let small = UIFont.systemFont(ofSize: 12 * s)
    label.font = small
    label.textColor = theme.color("muted")
    value.font = small
    value.textColor = theme.ink
    label.text = node["label"] as? String
    value.text = nil
    let width = bounds.width
    let accent = UIColor.transcriptHex(theme.style.accent)
    switch piece.kind {
    case "button":
      let button = control as? UIButton ?? UIButton(configuration: .gray())
      if control == nil {
        button.addAction(UIAction { [weak self] _ in self?.send?(NSNull()) }, for: .touchUpInside)
        adopt(button)
      }
      button.configuration?.title = node["label"] as? String
      button.configuration?.baseForegroundColor = theme.ink
      button.configuration?.cornerStyle = .medium
      button.isEnabled = node["disabled"] as? Bool != true
      label.text = nil
      button.frame = bounds
    case "toggle":
      let toggle = control as? UISwitch ?? UISwitch()
      if control == nil {
        toggle.addAction(UIAction { [weak self, weak toggle] _ in self?.send?(toggle?.isOn ?? false) }, for: .valueChanged)
        adopt(toggle)
      }
      toggle.onTintColor = accent
      if toggle.isOn != (node["value"] as? Bool == true) { toggle.setOn(node["value"] as? Bool == true, animated: true) }
      label.font = .systemFont(ofSize: 15 * s)
      label.textColor = theme.ink
      label.frame = CGRect(x: 0, y: 0, width: width - 64, height: bounds.height)
      toggle.frame.origin = CGPoint(x: width - toggle.bounds.width, y: (bounds.height - toggle.bounds.height) / 2)
      return
    case "slider":
      let slider = control as? UISlider ?? UISlider()
      let minimum = node["min"] as? Double ?? 0
      let maximum = node["max"] as? Double ?? 1
      let step = node["step"] as? Double ?? 0
      if control == nil {
        slider.addAction(UIAction { [weak self, weak slider] _ in
          guard let self, let slider else { return }
          var v = Double(slider.value)
          if step > 0 { v = (v / step).rounded() * step }
          self.value.text = VisualDrawing.number(v)
          self.throttled(min(maximum, max(minimum, v)), final: !slider.isTracking)
        }, for: [.valueChanged, .touchUpInside, .touchUpOutside])
        adopt(slider)
      }
      slider.minimumValue = Float(minimum)
      slider.maximumValue = Float(maximum)
      slider.minimumTrackTintColor = accent
      // The person's drag wins over a tree that arrives during it.
      if !slider.isTracking { slider.value = Float(node["value"] as? Double ?? minimum) }
      value.text = VisualDrawing.number(Double(slider.value))
      slider.frame = CGRect(x: 0, y: 18 * s, width: width, height: 36)
    case "input":
      let field = control as? UITextField ?? UITextField()
      if control == nil {
        field.borderStyle = .roundedRect
        field.clearButtonMode = .whileEditing
        field.addAction(UIAction { [weak self, weak field] _ in self?.send?(field?.text ?? "") }, for: .editingChanged)
        adopt(field)
      }
      field.placeholder = node["placeholder"] as? String
      field.textColor = theme.ink
      if !field.isFirstResponder { field.text = node["value"] as? String }
      field.frame = CGRect(x: 0, y: 18 * s + 4, width: width, height: 44)
    case "select":
      let button = control as? UIButton ?? UIButton(configuration: .gray())
      if control == nil {
        button.showsMenuAsPrimaryAction = true
        button.changesSelectionAsPrimaryAction = false
        adopt(button)
      }
      let options = node["options"] as? [String] ?? []
      let selected = node["selected"] as? Int ?? 0
      button.configuration?.title = selected < options.count ? options[selected] : ""
      button.configuration?.image = UIImage(systemName: "chevron.up.chevron.down")
      button.configuration?.imagePlacement = .trailing
      button.configuration?.imagePadding = 6
      button.configuration?.baseForegroundColor = theme.ink
      button.contentHorizontalAlignment = .fill
      button.menu = UIMenu(children: options.enumerated().map { index, option in
        UIAction(title: option, state: index == selected ? .on : .off) { [weak self] _ in self?.send?(index) }
      })
      button.frame = CGRect(x: 0, y: 18 * s + 4, width: width, height: 44)
    case "tabs":
      let labels = node["labels"] as? [String] ?? []
      var segments = control as? UISegmentedControl
      if segments == nil || segments!.numberOfSegments != labels.count {
        control?.removeFromSuperview()
        control = nil
        let made = UISegmentedControl(items: labels)
        made.apportionsSegmentWidthsByContent = true
        made.addAction(UIAction { [weak self, weak made] _ in self?.send?(made?.selectedSegmentIndex ?? 0) }, for: .valueChanged)
        adopt(made)
        segments = made
      }
      for (index, title) in labels.enumerated() { segments!.setTitle(title, forSegmentAt: index) }
      segments!.selectedSegmentIndex = node["selected"] as? Int ?? 0
      segments!.frame = CGRect(x: 0, y: 0, width: width, height: 34)
      label.text = nil
    default:
      break
    }
    label.frame = CGRect(x: 0, y: 0, width: width - 64, height: 16 * s)
    value.frame = CGRect(x: width - 64, y: 0, width: 64, height: 16 * s)
  }

  private func adopt(_ view: UIView) {
    control = view
    addSubview(view)
  }

  /// A drag sends at most every 50 ms, and always its last value.
  private func throttled(_ v: Any, final: Bool) {
    pending = v
    if final || Date().timeIntervalSince(lastSent) > 0.05 {
      lastSent = Date()
      pending = nil
      send?(v)
    } else {
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) { [weak self] in
        guard let self, let pending = self.pending else { return }
        self.pending = nil
        self.lastSent = Date()
        self.send?(pending)
      }
    }
  }
}

/// The visual's pieces as views, reused by path so controls survive updates.
final class TranscriptVisualView: UIView {
  private var views: [String: UIView] = [:]
  private let title = UILabel()
  private let reset = UIButton(type: .system)
  private let failure = UILabel()
  /// `handler`, `value` for a control; `nil` handler for reset.
  var onEvent: ((String?, Any) -> Void)?

  override init(frame: CGRect) {
    super.init(frame: frame)
    title.numberOfLines = 1
    addSubview(title)
    reset.setImage(UIImage(systemName: "arrow.counterclockwise"), for: .normal)
    reset.accessibilityLabel = "Reset visual"
    reset.addAction(UIAction { [weak self] _ in self?.onEvent?(nil, NSNull()) }, for: .touchUpInside)
    addSubview(reset)
    failure.numberOfLines = 0
    addSubview(failure)
  }
  required init?(coder: NSCoder) { fatalError() }

  func configure(_ model: TranscriptRowModel) {
    let theme = VisualTheme(style: model.style)
    // System controls follow the transcript's theme, not the device's.
    overrideUserInterfaceStyle = theme.dark ? .dark : .light
    let s = model.style.scale
    title.font = .systemFont(ofSize: 13 * s, weight: .semibold)
    title.textColor = theme.color("muted")
    title.text = model.visualTitle
    title.frame = CGRect(x: 0, y: 0, width: bounds.width - 40, height: 28)
    reset.tintColor = theme.color("muted")
    reset.frame = CGRect(x: bounds.width - 36, y: -4, width: 36, height: 36)
    failure.isHidden = model.visualError == nil
    failure.text = model.visualError
    failure.font = .systemFont(ofSize: 13 * s)
    failure.textColor = theme.color("negative")
    failure.frame = CGRect(x: 0, y: 32, width: bounds.width, height: max(0, bounds.height - 32))
    var used = Set<String>()
    for piece in model.visual?.pieces ?? [] {
      let key = piece.path + "/" + piece.kind
      used.insert(key)
      let frame = piece.rect.offsetBy(dx: 0, dy: 32)
      switch piece.kind {
      case "button", "toggle", "slider", "input", "select", "tabs":
        let view = views[key] as? VisualControl ?? VisualControl()
        if views[key] == nil { addSubview(view); views[key] = view }
        view.frame = frame
        let handler = piece.node["handler"] as? String ?? ""
        view.send = { [weak self] value in self?.onEvent?(handler, value) }
        view.configure(piece, theme: theme)
      case "scene":
        let view = views[key] as? VisualScene ?? VisualScene()
        if views[key] == nil { addSubview(view); views[key] = view }
        view.frame = frame
        view.theme = theme
        view.node = piece.node
      case "table":
        // A wide table scrolls sideways inside the visual, not the page.
        let scroll = views[key] as? UIScrollView ?? UIScrollView()
        if views[key] == nil {
          scroll.showsHorizontalScrollIndicator = false
          scroll.addSubview(VisualDrawing())
          addSubview(scroll)
          views[key] = scroll
        }
        scroll.frame = frame
        let columns = (piece.node["columns"] as? [Any] ?? []).count
        let width = max(frame.width, CGFloat(columns) * 112)
        scroll.contentSize = CGSize(width: width, height: frame.height)
        if let drawing = scroll.subviews.first as? VisualDrawing {
          drawing.frame = CGRect(x: 0, y: 0, width: width, height: frame.height)
          drawing.piece = piece
          drawing.theme = theme
          drawing.setNeedsDisplay()
        }
      default:
        let view = views[key] as? VisualDrawing ?? VisualDrawing()
        if views[key] == nil {
          view.isUserInteractionEnabled = false
          // Cards sit behind what they hold.
          if piece.kind == "card" { insertSubview(view, at: 0) } else { addSubview(view) }
          views[key] = view
        }
        view.frame = frame
        view.piece = piece
        view.theme = theme
        view.setNeedsDisplay()
      }
    }
    for (key, view) in views where !used.contains(key) {
      view.removeFromSuperview()
      views.removeValue(forKey: key)
    }
  }
}
