import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../l10n/zh.dart';
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
    this.cancelled = false,
    this.outcomeUnknown = false,
  });

  final String title, message, details;
  final String? code;
  final bool cancelled, outcomeUnknown;
  bool get retryable => !cancelled && !outcomeUnknown;

  static DshError describe(Object error, {String? operation}) {
    if (error is DshError) return error;
    final structured = error is DshException ? error : null;
    final raw = error.toString();
    // Legacy state stores exceptions as strings; retain their error code while
    // new callers keep the exception itself.
    final code =
        structured?.code ??
        RegExp(r'\(([a-z][a-z0-9-]+)\)(?=；|\s|$)').firstMatch(raw)?.group(1);
    final unknown =
        structured?.outcomeUnknown == true || raw.contains('操作结果尚未确认');
    final cancelled = const {
      'cancelled',
      'canceled',
      'request-cancelled',
    }.contains(code);
    var message = unknown
        ? DshZh.outcomeUnknown
        : cancelled
        ? DshZh.operationCancelled
        : switch (code) {
            'transport' ||
            'connection' ||
            'connection-closed' => DshZh.connectionFailure,
            'timeout' || 'http-408' || 'http-504' => DshZh.requestTimeout,
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
                  : structured != null
                  ? redact(structured.message)
                  : error is FormatException
                  ? redact(error.message)
                  : error is StateError &&
                        error.message.toString().length < 500 &&
                        RegExp(r'[\u4e00-\u9fff]')
                            .hasMatch(error.message.toString())
                  ? redact(error.message.toString())
                  : error is String &&
                        raw.length < 500 &&
                        !RegExp(r'(^|\n)#\d+\s|Stack trace:').hasMatch(raw)
                  ? redact(raw)
                  : DshZh.unknownError,
          };
    final businessMessage = redact(structured?.message ?? raw);
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
      cancelled: cancelled,
      outcomeUnknown: unknown,
      details: redact(
        [
          if (code != null) 'code: $code',
          raw,
          if (structured != null && structured.details.isNotEmpty)
            const JsonEncoder.withIndent('  ').convert(structured.details),
        ].join('\n'),
      ),
    );
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
