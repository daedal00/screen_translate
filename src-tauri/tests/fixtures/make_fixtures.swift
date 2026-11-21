// Regenerate: swiftc make_fixtures.swift -o /tmp/mkfix && /tmp/mkfix .   (or `swift make_fixtures.swift .`)
// Renders black text on white using macOS system fonts. Strings below are the source of truth
// and must match the EXPECTED constants in ../ocr.rs.
import AppKit

let fixtures: [(name: String, font: String, lines: [String])] = [
    ("en", "Helvetica Neue", ["Hello world", "Good morning"]),
    ("de", "Helvetica Neue", ["Schöne Grüße", "Straße nach Köln"]),
    ("ja", "Hiragino Sans", ["こんにちは世界", "今日は天気です"]),
    ("zh", "PingFang SC", ["你好世界", "今天天气很好"]),
    ("ko", "Apple SD Gothic Neo", ["안녕하세요", "오늘 날씨가 좋아요"]),
]

let outDir = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "."
let size: CGFloat = 40
let pad: CGFloat = 24

for f in fixtures {
    guard let font = NSFont(name: f.font, size: size) else { fatalError("missing font \(f.font)") }
    let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.black]
    let lineH = ceil(size * 1.5)
    let width = Int(ceil(f.lines.map { ($0 as NSString).size(withAttributes: attrs).width }.max()!) + pad * 2)
    let height = Int(lineH * CGFloat(f.lines.count) + pad * 2)
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSColor.white.setFill()
    NSRect(x: 0, y: 0, width: width, height: height).fill()
    for (i, line) in f.lines.enumerated() {
        let y = CGFloat(height) - pad - lineH * CGFloat(i + 1) + (lineH - size) / 4
        (line as NSString).draw(at: NSPoint(x: pad, y: y), withAttributes: attrs)
    }
    NSGraphicsContext.restoreGraphicsState()
    let png = rep.representation(using: .png, properties: [:])!
    try! png.write(to: URL(fileURLWithPath: "\(outDir)/\(f.name).png"))
}
