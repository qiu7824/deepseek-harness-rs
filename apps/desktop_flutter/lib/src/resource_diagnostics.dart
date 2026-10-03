/// Numeric resource ownership only; never include paths, drafts or message text.
abstract interface class ResourceDiagnostics {
  Map<String, int> get resourceDiagnostics;
}

/// A lifecycle identity, unrelated to a file path, session ID or user content.
/// The generated value remains stable for one mounted owner only.
abstract interface class ScopedResourceDiagnostics
    implements ResourceDiagnostics {
  int get resourceScopeId;
  String get resourceScopeKind;
}

mixin ResourceDiagnosticScope implements ScopedResourceDiagnostics {
  static int _nextScopeId = 0;
  @override
  final int resourceScopeId = ++_nextScopeId;
}
