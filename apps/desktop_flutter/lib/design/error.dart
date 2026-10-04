import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../l10n/zh.dart';
import '../l10n/runtime_zh.dart';
import 'primitives.dart';
import 'motion.dart';

import 'package:dsh_desktop/design/typography.dart';

/// User-facing recovery and redacted technical details for a failed operation.
class DshError {
  const DshError({
    required this.title,
    required this.message,
    required this.details,
    this.code,
    this.status,
    this.traceId,
    this.cancelled = false,
    this.outcomeUnknown = false,
  });

  final String title, message, details;
  final String? code;
  final int? status;
  final String? traceId;
  final bool cancelled, outcomeUnknown;
  bool get retryable => !cancelled && !outcomeUnknown && status != 400;

  static DshError describe(Object error, {String? operation}) {
    if (error is DshError) return error;
    final structured = error is DshException ? error : null;
    final raw = error.toString();
    final failure =
        DshErrorInfo.tryParse(structured?.message ?? error) ??
        (error is Exception ? _startupExceptionEnvelope(raw) : null) ??
        (structured == null
            ? null
            : DshErrorInfo.tryParse({
                'code': structured.code,
                'message': structured.message,
                'details': structured.details,
              }, errorObject: true));
    final startup = failure != null && _isStartupEnvelope(failure)
        ? failure : null;
    // Legacy state stores exceptions as strings; retain their error code while
    // new callers keep the exception itself.
    final code =
        structured?.code ??
        failure?.code ??
        RegExp(r'\(([A-Za-z][A-Za-z0-9_-]+)\)(?=；|\s|$)')
            .firstMatch(raw)?.group(1);
    final status =
        DshErrorInfo.statusFrom(structured?.details['httpStatus']) ??
        DshErrorInfo.statusFrom(
          RegExp(r'^http-(\d{3})$')
              .firstMatch(structured?.code ?? code ?? '')?.group(1),
        ) ??
        failure?.status;
    final traceId = failure?.traceId;
    final originalMessage = failure?.message ?? structured?.message ?? raw;
    var serializedJson = false;
    if (error is String) {
      if (raw.length <= 65536) {
        try {
          jsonDecode(raw);
          serializedJson = true;
        } on FormatException {
          // A plain failure message remains suitable for human presentation.
        }
      } else {
        serializedJson = RegExp(r'^\s*[\[{"]').hasMatch(raw);
      }
    }
    final plainError = structured != null || (error is String && !serializedJson);
    final recognizedFailure = failure != null || structured != null || (plainError && code != null);
    final sourceMessage = startup != null ? originalMessage : recognizedFailure
        ? _primaryDevinMessage(
            originalMessage,
            legacyCode: failure == null && structured == null ? code : null,
          )
        : originalMessage;
    final providerUnavailable =
        startup == null &&
        (failure != null || plainError) &&
        !const {'timeout', 'TIMEOUT', 'http-408', 'http-504'}.contains(code) &&
        RegExp(
          r'\b(?:third-party )?model provider\b.*\b(?:not available|unavailable)\b',
          caseSensitive: false,
          dotAll: true,
        ).hasMatch(sourceMessage);
    final unknown =
        structured?.outcomeUnknown == true ||
        (startup == null && raw.contains('操作结果尚未确认'));
    final cancelled = const {
      'cancelled',
      'canceled',
      'request-cancelled',
      'CANCELLED',
    }.contains(code);
    var message = unknown
        ? DshZh.outcomeUnknown
        : cancelled
        ? DshZh.operationCancelled
        : providerUnavailable
        ? '模型提供方返回暂不可用，本次请求未完成。请稍后再试，或切换其他模型。'
        : switch (code) {
            'transport' ||
            'connection' ||
            'connection-closed' => DshZh.connectionFailure,
            'timeout' || 'TIMEOUT' || 'http-408' || 'http-504' => DshZh.requestTimeout,
            'conflict' ||
            'revision-conflict' ||
            'title-conflict' ||
            'http-409' => DshZh.conflict,
            'permission-denied' ||
            'forbidden' ||
            'http-401' ||
            'http-403' => DshZh.permissionDenied,
            'not-found' || 'http-404' => DshZh.missingResource,
            _ =>
              error is TimeoutException
                  ? DshZh.requestTimeout
                  : error is SocketException
                  ? DshZh.connectionFailure
                  : failure != null || structured != null
                  ? redact(sourceMessage)
                  : error is FormatException
                  ? redact(error.message)
                  : error is StateError &&
                        error.message.toString().length < 500 &&
                        RegExp(r'[\u4e00-\u9fff]')
                            .hasMatch(error.message.toString())
                  ? redact(error.message.toString())
                  : error is String &&
                        sourceMessage.length < 500 &&
                        !RegExp(r'(^|\n)#\d+\s|Stack trace:').hasMatch(sourceMessage)
                  ? redact(sourceMessage)
                  : DshZh.unknownError,
          };
    final businessMessage = redact(sourceMessage);
    if (!unknown &&
        !cancelled &&
        code != null &&
        RegExp(r'[\u4e00-\u9fff]').hasMatch(businessMessage) &&
        !message.contains(businessMessage)) {
      message = '$businessMessage\n$message';
    }
    return DshError(
      title: cancelled
          ? DshZh.operationCancelled
          : operation == null
          ? DshZh.errorTitle
          : DshZh.operationFailed(operation),
      message: message,
      code: code,
      status: status,
      traceId: traceId,
      cancelled: cancelled,
      outcomeUnknown: unknown,
      details: redact(
        [
          if (code != null) 'code: $code',
          if (status != null) 'status: $status',
          if (traceId != null) 'trace ID: $traceId',
          if (startup == null) raw,
          if (startup != null) ...[
            startup.message,
            if (startup.details['exitCode'] != null)
              'exit code: ${startup.details['exitCode']}',
            DshRuntimeZh.hostStartupLogLocation(startup.details['logFile'] as String),
            startup.details['diagnostics'] as String,
          ],
          if (structured != null && structured.details.isNotEmpty)
            const JsonEncoder.withIndent('  ').convert(structured.details),
        ].join('\n'),
      ),
    );
  }

  static bool _isStartupEnvelope(DshErrorInfo failure) =>
      failure.envelope['kind'] == 'error' &&
      failure.envelope['error'] is Map &&
      failure.code == 'host-startup' &&
      failure.details['logFile'] is String &&
      failure.details['diagnostics'] is String &&
      (failure.details['exitCode'] == null || failure.details['exitCode'] is int);

  static DshErrorInfo? _startupExceptionEnvelope(String raw) {
    // Exceptions may serialize the declared error envelope; do not interpret
    // their arbitrary toString JSON as an error or promote business metadata.
    final failure = DshErrorInfo.tryParse(raw);
    return failure != null && _isStartupEnvelope(failure) ? failure : null;
  }

  /// Only a bounded, complete Rust request-shape appendix is presentation-only.
  /// The original exception/history text remains intact in folded details.
  static String _primaryDevinMessage(String message, {String? legacyCode}) {
    const marker = '\n[devin-diagnostic:';
    var body = message;
    var legacySuffix = '';
    if (legacyCode != null && body.endsWith(' ($legacyCode)')) {
      legacySuffix = ' ($legacyCode)';
      body = body.substring(0, body.length - legacySuffix.length);
    }
    var nativeSuffix = '';
    final native = RegExp(
      r' \[(?:canceled|unknown|invalid_argument|deadline_exceeded|not_found|already_exists|permission_denied|resource_exhausted|failed_precondition|aborted|out_of_range|unimplemented|internal|unavailable|data_loss|unauthenticated)\]$',
    ).firstMatch(body);
    if (native != null) {
      nativeSuffix = native.group(0)!;
      body = body.substring(0, native.start);
    }
    final start = body.lastIndexOf(marker);
    if (start <= 0 ||
        !body.endsWith(']') ||
        body.length - start - marker.length - 1 > 4096) {
      return message;
    }
    final appendix = body.substring(start + marker.length, body.length - 1);
    if (appendix.contains('\n') || appendix.contains('\r')) {
      return message;
    }
    Object? summary;
    try {
      summary = jsonDecode(appendix);
    } on FormatException {
      return message;
    }
    if (summary is! Map || !_devinSummary(summary)) {
      return message;
    }
    return '${body.substring(0, start)}$nativeSuffix$legacySuffix';
  }

  static bool _devinSummary(Map<dynamic, dynamic> summary) {
    const requiredKeys = {
      'phase',
      'modelUidHash',
      'modelUidChars',
      'toolCount',
      'messageCount',
      'signedReplayCount',
      'schema',
      'requestBytes',
      'compressedBytes',
    };
    if (!requiredKeys.every(summary.containsKey) ||
        summary.keys.any(
          (key) => !requiredKeys.contains(key) && key != 'badRequestFields',
        ) ||
        !const {'chat-http', 'chat-connect'}.contains(summary['phase'])) {
      return false;
    }
    bool count(Object? value) => value is int && value >= 0;
    bool hash(Object? value) =>
        value is String && RegExp(r'^[0-9a-f]{64}$').hasMatch(value);
    if (!hash(summary['modelUidHash']) ||
        !const [
          'modelUidChars',
          'toolCount',
          'messageCount',
          'signedReplayCount',
          'requestBytes',
          'compressedBytes',
        ].every((key) => count(summary[key]))) {
      return false;
    }
    final schema = summary['schema'];
    if (schema is! Map ||
        schema.length != 4 ||
        !const [
          'objectRoots',
          'rootCombinators',
          'bytes',
        ].every((key) => count(schema[key])) ||
        !hash(schema['sha256'])) {
      return false;
    }
    if (summary.containsKey('badRequestFields')) {
      final fields = summary['badRequestFields'];
      if (fields is! List || fields.length > 4) {
        return false;
      }
      for (final field in fields) {
        if (field is! Map || field.length != 1) {
          return false;
        }
        if (field.containsKey('pathHash')) {
          if (!hash(field['pathHash'])) {
            return false;
          }
        } else {
          final path = field['path'];
          if (path is! String ||
              path.length > 96 ||
              !RegExp(
                r'^(?:model|chat_model_uid|chatModelUid|configuration|tools|chat_message_prompts|chatMessagePrompts|prompt|metadata|tool_choice|toolChoice|system_prompt_cache_options|systemPromptCacheOptions)(?:\[[0-9]{1,4}\])?(?:\.[A-Za-z_][A-Za-z_0-9]*(?:\[[0-9]{1,4}\])?)*$',
              ).hasMatch(path)) {
            return false;
          }
        }
      }
    }
    return true;
  }

  static String redact(String value) => value
      .replaceAllMapped(
        RegExp(
          r'''(["']?(?:authorization|proxy-authorization|cookie|set-cookie)["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|(?:(?:Bearer|Basic)\s+)?[^\r\n,]+)''',
          caseSensitive: false,
        ),
        (m) => '${m[1]}[已隐藏]',
      )
      .replaceAllMapped(
        RegExp(
          r'''(["']?(?:api[-_]?key|access[-_]?token|refresh[-_]?token|token|password|secret|credential)["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s&,;}]+)''',
          caseSensitive: false,
        ),
        (m) => '${m[1]}[已隐藏]',
      )
      .replaceAll(
        RegExp(
          r'\b(?:Bearer|Basic)\s+[A-Za-z0-9._~+/-]+=*',
          caseSensitive: false,
        ),
        '[已隐藏]',
      )
      .replaceAllMapped(
        RegExp(r'(https?://)[^\s/@]+:[^\s/@]+@'),
        (m) => '${m[1]}[已隐藏]@',
      );
}

/// Keep transient errors short; reveal redacted diagnostics only on request.
void showDshError(BuildContext context, Object error, {String? operation}) {
  if (!context.mounted) return;
  final description = DshError.describe(error, operation: operation);
  void details() {
    if (!context.mounted) return;
    ShadToaster.maybeOf(context)?.hide(animate: false);
    showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text(description.title),
        content: SizedBox(
          width: 520,
          child: SingleChildScrollView(child: DshErrorView(error: description)),
        ),
        actions: [
          DshButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text(DshZh.closeNotice),
          ),
        ],
      ),
    );
  }

  final message = Text(
    description.message,
    maxLines: 3,
    overflow: TextOverflow.ellipsis,
  );
  final messenger = ScaffoldMessenger.maybeOf(context);
  if (messenger != null) {
    messenger.showSnackBar(
      SnackBar(
        content: message,
        action: SnackBarAction(label: DshZh.viewDetails, onPressed: details),
      ),
    );
    return;
  }
  final toaster = ShadToaster.maybeOf(context);
  if (toaster == null) {
    details();
    return;
  }
  final tokens = DshTokens.of(context);
  toaster.show(
    ShadToast(
      title: Text(description.title),
      description: message,
      titleStyle: DshTypography.body.copyWith(
        color: tokens.error.foreground,
        fontWeight: FontWeight.w600,
      ),
      descriptionStyle: DshTypography.body.copyWith(
        color: tokens.error.foreground,
      ),
      backgroundColor: tokens.error.background,
      animateIn: DshMotion.disabled(context) ? const [] : null,
      animateOut: DshMotion.disabled(context) ? const [] : null,
      action: DshButton(
        onPressed: details,
        child: const Text(DshZh.viewDetails),
      ),
    ),
  );
}

/// Error content is concise by default; diagnostics never expand implicitly.
class DshErrorView extends StatelessWidget {
  const DshErrorView({
    super.key,
    required this.error,
    this.operation,
    this.onRetry,
    this.onDismiss,
  });
  final Object error;
  final String? operation;
  final VoidCallback? onRetry, onDismiss;

  @override
  Widget build(BuildContext context) {
    final description = DshError.describe(error, operation: operation);
    final colors = Theme.of(context).colorScheme;
    return Semantics(
      liveRegion: true,
      child: Material(
        color: description.cancelled
            ? DshColors(context).layer
            : colors.errorContainer,
        borderRadius: BorderRadius.circular(8),
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                description.message,
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: DshTypography.sizeBody,
                  height: 1.55,
                  color: description.cancelled
                      ? colors.onSurface
                      : colors.onErrorContainer,
                ),
              ),
              if (onRetry != null || onDismiss != null)
                Wrap(
                  spacing: 8,
                  children: [
                    if (onRetry != null && description.retryable)
                      DshButton(
                        onPressed: onRetry,
                        child: const Text(DshZh.retry),
                      ),
                    if (onDismiss != null)
                      DshButton(
                        onPressed: onDismiss,
                        child: const Text(DshZh.closeNotice),
                      ),
                  ],
                ),
              ExpansionTile(
                key: ValueKey(description.details),
                tilePadding: EdgeInsets.zero,
                childrenPadding: EdgeInsets.zero,
                dense: true,
                title: const Text(
                  DshZh.details,
                  style: TextStyle(fontSize: DshTypography.sizeCaption),
                ),
                children: [
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxHeight: 180),
                    child: SingleChildScrollView(
                      child: Align(
                        alignment: Alignment.centerLeft,
                        child: SelectableText(
                          description.details,
                          style: const TextStyle(
                            fontSize: DshTypography.sizeCaption,
                            height: 1.5,
                          ),
                        ),
                      ),
                    ),
                  ),
                  Align(
                    alignment: Alignment.centerRight,
                    child: DshButton(
                      onPressed: () => Clipboard.setData(
                        ClipboardData(text: description.details),
                      ),
                      child: const Text(DshZh.copyDetails),
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}
