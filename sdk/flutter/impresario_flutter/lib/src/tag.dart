import 'package:flutter/widgets.dart';

/// Adds a stable automation id to a widget subtree.
///
/// The id is exposed through `Semantics(identifier:)`, which Flutter maps to
/// Android `resource-id`, iOS `accessibilityIdentifier` and the web
/// `flt-semantics-identifier` attribute on supported versions. The inspector
/// turns it into a `testid=` locator, so it should be stable, unique and
/// independent of user-facing text.
///
/// Prefer this over `Key`: keys are not exposed to the native accessibility
/// layer and therefore cannot be resolved by a runner.
class ImpresarioTag extends StatelessWidget {
  const ImpresarioTag({
    required this.id,
    required this.child,
    this.container = true,
    this.explicitChildNodes = false,
    super.key,
  }) : _useLabelFallback = false,
       assert(id != '');

  /// Label-based fallback for Flutter versions or native layers that do not
  /// expose semantics identifiers.
  ///
  /// This changes the accessible name, so use it only in QA/automation builds.
  /// The inspector matches it as `label=<id>` instead of `testid=<id>`.
  const ImpresarioTag.labelFallback({
    required this.id,
    required this.child,
    this.container = true,
    this.explicitChildNodes = false,
    super.key,
  }) : _useLabelFallback = true,
       assert(id != '');

  /// Stable automation id.
  final String id;

  /// Tagged subtree.
  final Widget child;

  /// Whether to create a dedicated semantics container.
  ///
  /// Keeping the default (`true`) guarantees that the identifier gets its own
  /// node even when the child does not create one (for example a plain text).
  final bool container;

  /// Whether the child's semantics should be explicit children of this node.
  final bool explicitChildNodes;

  final bool _useLabelFallback;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      identifier: _useLabelFallback ? null : id,
      label: _useLabelFallback ? id : null,
      container: container,
      explicitChildNodes: explicitChildNodes,
      child: child,
    );
  }
}
