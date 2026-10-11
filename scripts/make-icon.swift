#!/usr/bin/env swift
// App icon generator: draws a rounded indigo→violet tile with a white
// sound-wave glyph, renders every iconset size, and builds an .icns.
// Usage: swift scripts/make-icon.swift <out.icns>
import CoreGraphics
import Foundation
import ImageIO

func fail(_ message: String) -> Never {
    if let data = (message + "\n").data(using: .utf8) {
        FileHandle.standardError.write(data)
    }
    exit(1)
}

guard CommandLine.arguments.count == 2 else {
    fail("Usage: swift scripts/make-icon.swift <out.icns>")
}
let outPath = CommandLine.arguments[1]
let outURL = URL(fileURLWithPath: outPath)

guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) else {
    fail("make-icon: couldn't create sRGB color space")
}

/// Draw the icon at `size`×`size` pixels.
func drawIcon(size: Int) -> CGImage? {
    let width = size
    let height = size
    guard
        let context = CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: 0, space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
    else {
        return nil
    }
    let side = CGFloat(size)
    context.clear(CGRect(x: 0, y: 0, width: side, height: side))

    // Squircle tile, about 10% inset on each side.
    let inset = side * 0.10
    let tile = CGRect(x: inset, y: inset, width: side - inset * 2, height: side - inset * 2)
    let radius = tile.width * 0.225
    let tilePath = CGPath(
        roundedRect: tile, cornerWidth: radius, cornerHeight: radius, transform: nil)

    context.saveGState()
    context.addPath(tilePath)
    context.clip()

    // Diagonal gradient: deep indigo (top-left) to violet (bottom-right).
    let indigo = CGColor(srgbRed: 0.20, green: 0.18, blue: 0.62, alpha: 1)
    let violet = CGColor(srgbRed: 0.48, green: 0.22, blue: 0.92, alpha: 1)
    if let gradient = CGGradient(
        colorsSpace: colorSpace, colors: [indigo, violet] as CFArray, locations: [0, 1])
    {
        context.drawLinearGradient(
            gradient,
            start: CGPoint(x: tile.minX, y: tile.maxY),
            end: CGPoint(x: tile.maxX, y: tile.minY),
            options: [])
    }

    // Subtle top highlight fading out halfway down.
    let highlightTop = CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.28)
    let highlightClear = CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0)
    if let highlight = CGGradient(
        colorsSpace: colorSpace, colors: [highlightTop, highlightClear] as CFArray,
        locations: [0, 1])
    {
        context.drawLinearGradient(
            highlight,
            start: CGPoint(x: 0, y: tile.maxY),
            end: CGPoint(x: 0, y: tile.maxY - tile.height * 0.5),
            options: [])
    }

    // Centered white sound-wave glyph: 5 rounded vertical bars.
    let white = CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 1)
    context.setFillColor(white)
    let heights: [CGFloat] = [0.42, 0.68, 1.0, 0.68, 0.42]
    let maxBarHeight = tile.height * 0.52
    let barWidth = tile.width * 0.095
    let gap = barWidth * 0.75
    let totalWidth = barWidth * CGFloat(heights.count) + gap * CGFloat(heights.count - 1)
    var x = tile.midX - totalWidth / 2
    for fraction in heights {
        let barHeight = maxBarHeight * fraction
        let bar = CGRect(
            x: x, y: tile.midY - barHeight / 2, width: barWidth, height: barHeight)
        let barPath = CGPath(
            roundedRect: bar, cornerWidth: barWidth / 2, cornerHeight: barWidth / 2,
            transform: nil)
        context.addPath(barPath)
        context.fillPath()
        x += barWidth + gap
    }
    context.restoreGState()

    // Faint edge stroke for definition on light backgrounds.
    context.addPath(tilePath)
    context.setStrokeColor(CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 0.12))
    context.setLineWidth(max(1, side / 512))
    context.strokePath()

    return context.makeImage()
}

func writePNG(_ image: CGImage, to url: URL) -> Bool {
    guard
        let destination = CGImageDestinationCreateWithURL(
            url as CFURL, "public.png" as CFString, 1, nil)
    else {
        return false
    }
    CGImageDestinationAddImage(destination, image, nil)
    return CGImageDestinationFinalize(destination)
}

// All iconset sizes (name, pixel size).
let sizes: [(String, Int)] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]

let fileManager = FileManager.default
let iconsetURL = fileManager.temporaryDirectory
    .appendingPathComponent("SmartVoicesAppIcon.iconset", isDirectory: true)
try? fileManager.removeItem(at: iconsetURL)
do {
    try fileManager.createDirectory(at: iconsetURL, withIntermediateDirectories: true)
} catch {
    fail("make-icon: couldn't create \(iconsetURL.path): \(error.localizedDescription)")
}

for (name, pixels) in sizes {
    guard let image = drawIcon(size: pixels) else {
        fail("make-icon: couldn't render \(pixels)px icon")
    }
    let url = iconsetURL.appendingPathComponent(name)
    if !writePNG(image, to: url) {
        fail("make-icon: couldn't write \(url.path)")
    }
}

let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconsetURL.path, "-o", outURL.path]
do {
    try iconutil.run()
} catch {
    fail("make-icon: couldn't run iconutil: \(error.localizedDescription)")
}
iconutil.waitUntilExit()
try? fileManager.removeItem(at: iconsetURL)
if iconutil.terminationStatus != 0 {
    fail("make-icon: iconutil failed with status \(iconutil.terminationStatus)")
}
print(outURL.path)
