/// Numeric resource ownership only; never include paths, drafts or message text.
abstract interface class ResourceDiagnostics {
  Map<String, int> get resourceDiagnostics;
}
