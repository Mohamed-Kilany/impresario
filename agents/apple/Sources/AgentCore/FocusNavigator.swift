import Foundation

/// What the navigator may ask of the device (spec 6.3, `FocusProbe`).
public protocol FocusProbe {
    func currentFocusLabel() -> String?
    func currentFocusFrame() -> CGRect?
    func targetFrame() -> CGRect?
    func press(_ direction: Direction) -> Bool
    func isTargetFocused() -> Bool
}

/// One step of the focus trail reported in `details.focus_trail` (spec 6.3).
public struct FocusTrailEntry: Sendable, Equatable {
    public let step: Int
    public let direction: String
    public let focusedLabel: String
    public let frame: String

    public init(step: Int, direction: String, focusedLabel: String, frame: String) {
        self.step = step
        self.direction = direction
        self.focusedLabel = focusedLabel
        self.frame = frame
    }
}

public enum FocusResult: Sendable, Equatable {
    case focused

    /// `reason` is one of `budget`, `cycle`, `blocked` (spec 6.3).
    case unreachable(reason: String, trail: [FocusTrailEntry], steps: Int)
}

/// D-pad traversal for TV platforms (spec 6.3): move the focus towards the
/// target along the dominant axis, guard against cycles, and report an honest
/// trail when the target cannot be reached. Pure logic, unit-tested with a fake
/// probe.
public struct FocusNavigator {
    private let probe: FocusProbe
    private let maxSteps: Int

    public init(probe: FocusProbe, maxSteps: Int) {
        self.probe = probe
        self.maxSteps = maxSteps
    }

    public func ensureTargetFocused() -> FocusResult {
        if probe.isTargetFocused() { return .focused }

        var trail: [FocusTrailEntry] = []
        var seen = Set<String>()
        var step = 0

        while step < maxSteps {
            guard let current = probe.currentFocusFrame() else {
                // Unreadable focus: one neutral `down` press, then retry.
                step += 1
                trail.append(entry(step: step, direction: .down, frame: nil))
                if !seen.insert("none|down").inserted {
                    return .unreachable(reason: "cycle", trail: trail, steps: step)
                }
                if !probe.press(.down) {
                    return .unreachable(reason: "blocked", trail: trail, steps: step)
                }
                if probe.isTargetFocused() { return .focused }
                continue
            }

            guard let target = probe.targetFrame() else {
                return .unreachable(reason: "blocked", trail: trail, steps: step)
            }

            let dx = target.midX - current.midX
            let dy = target.midY - current.midY
            let direction: Direction
            if dy == 0 && dx == 0 {
                direction = .down
            } else if abs(dy) >= abs(dx) {
                direction = dy > 0 ? .down : .up
            } else {
                direction = dx > 0 ? .right : .left
            }

            let frameKey = frameString(current)
            if !seen.insert("\(frameKey)|\(direction.rawValue)").inserted {
                return .unreachable(reason: "cycle", trail: trail, steps: step)
            }
            step += 1
            trail.append(entry(step: step, direction: direction, frame: current))
            if !probe.press(direction) {
                return .unreachable(reason: "blocked", trail: trail, steps: step)
            }

            if probe.isTargetFocused() { return .focused }

            if let after = probe.currentFocusFrame(), after == current {
                // The dominant axis is blocked: try the other axis once.
                let alternative: Direction
                switch direction {
                case .up, .down:
                    alternative = dx >= 0 ? .right : .left
                case .left, .right:
                    alternative = dy >= 0 ? .down : .up
                }
                if !seen.insert("\(frameKey)|\(alternative.rawValue)").inserted {
                    return .unreachable(reason: "cycle", trail: trail, steps: step)
                }
                if step >= maxSteps { break }
                step += 1
                trail.append(entry(step: step, direction: alternative, frame: current))
                if !probe.press(alternative) {
                    return .unreachable(reason: "blocked", trail: trail, steps: step)
                }
                if probe.isTargetFocused() { return .focused }
                if let afterAlternative = probe.currentFocusFrame(), afterAlternative == current {
                    trail.append(
                        FocusTrailEntry(
                            step: step,
                            direction: "blocked",
                            focusedLabel: probe.currentFocusLabel() ?? "",
                            frame: frameString(current)
                        )
                    )
                    return .unreachable(reason: "blocked", trail: trail, steps: step)
                }
            }
        }
        return .unreachable(reason: "budget", trail: trail, steps: step)
    }

    private func entry(step: Int, direction: Direction, frame: CGRect?) -> FocusTrailEntry {
        FocusTrailEntry(
            step: step,
            direction: direction.rawValue,
            focusedLabel: probe.currentFocusLabel() ?? "",
            frame: frame.map(frameString) ?? ""
        )
    }

    /// `[left,top][right,bottom]` — the same shape the Android agent uses.
    public static func frameString(_ rect: CGRect) -> String {
        "[\(Int(rect.minX)),\(Int(rect.minY))][\(Int(rect.maxX)),\(Int(rect.maxY))]"
    }

    private func frameString(_ rect: CGRect) -> String {
        FocusNavigator.frameString(rect)
    }

    /// Serializes a trail into the `details.focus_trail` JSON array (spec 6.3).
    public static func trailJson(_ trail: [FocusTrailEntry]) -> String {
        let entries = trail.map { entry in
            "{\"step\":\(entry.step)," +
                "\"direction\":\(jsonString(entry.direction))," +
                "\"focused_label\":\(jsonString(entry.focusedLabel))," +
                "\"frame\":\(jsonString(entry.frame))}"
        }
        return "[" + entries.joined(separator: ",") + "]"
    }

    private static func jsonString(_ value: String) -> String {
        var out = "\""
        for ch in value.unicodeScalars {
            switch ch {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            default:
                if ch.value < 0x20 {
                    out += String(format: "\\u%04x", ch.value)
                } else {
                    out.unicodeScalars.append(ch)
                }
            }
        }
        return out + "\""
    }
}
