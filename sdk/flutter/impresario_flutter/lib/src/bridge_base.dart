/// Optional loopback bridge used when no VM service is available (release
/// builds) or when the host prefers a plain TCP channel.
abstract interface class ImpresarioBridge {
  Future<void> start();

  Future<void> stop();

  /// Actual bound port, or `null` on web.
  int? get port;

  /// Pushes a `semantics` event to every connected client.
  void notifyChanged(int rev);
}
