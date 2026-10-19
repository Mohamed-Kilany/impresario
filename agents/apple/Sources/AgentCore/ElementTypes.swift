import Foundation

/// `XCUIElement.ElementType` names accepted by the `type:` selector key on
/// Apple platforms (spec 5.3: lowerCamel names; an unknown name is a selector
/// error). Kept as an own enum so `AgentCore` never imports XCTest — the
/// driver maps the case onto `XCUIElement.ElementType`.
public enum AgentElementType: String, Sendable, Equatable, CaseIterable {
    case any
    case other
    case application
    case group
    case window
    case sheet
    case drawer
    case alert
    case dialog
    case button
    case radioButton
    case radioGroup
    case checkBox
    case disclosureTriangle
    case popUpButton
    case comboBox
    case menuButton
    case toolbarButton
    case popover
    case keyboard
    case key
    case navigationBar
    case tabBar
    case tabGroup
    case toolbar
    case statusBar
    case table
    case tableRow
    case tableColumn
    case outline
    case outlineRow
    case browser
    case collectionView
    case cell
    case slider
    case pageIndicator
    case progressIndicator
    case activityIndicator
    case segmentedControl
    case picker
    case pickerWheel
    case `switch`
    case toggle
    case link
    case image
    case icon
    case searchField
    case scrollView
    case scrollBar
    case staticText
    case textField
    case secureTextField
    case datePicker
    case textView
    case menu
    case menuItem
    case menuBar
    case menuBarItem
    case map
    case webView
    case incrementArrow
    case decrementArrow
    case timeline
    case ratingIndicator
    case valueIndicator
    case splitter
    case tab
    case touchBar
    case statusItem
}
