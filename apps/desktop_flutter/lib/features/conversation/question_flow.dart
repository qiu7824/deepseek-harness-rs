import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/rich_content.dart';
import '../../src/controller.dart';

class QuestionFlow extends StatefulWidget {
  const QuestionFlow({
    super.key,
    required this.controller,
    required this.frame,
  });
  final DesktopController controller;
  final HostFrame frame;
  @override
  State<QuestionFlow> createState() => _QuestionFlowState();
}

class _QuestionFlowState extends State<QuestionFlow> {
  final selected = <String, Set<String>>{};
  final custom = <String, String>{};
  final skipped = <String>{};
  final input = TextEditingController(), scroll = ScrollController();
  final inputFocus = FocusNode();
  int index = 0;
  String? busy, error;
  List<Json> get questions => objects(widget.frame.payload['questions']);
  bool get enabled =>
      busy == null &&
      widget.controller.connected &&
      !widget.controller.answering.contains(widget.frame.rpcId);
  bool answered(String id) =>
      (selected[id]?.isNotEmpty ?? false) ||
      (custom[id]?.trim().isNotEmpty ?? false);

  @override
  void dispose() {
    input.dispose();
    inputFocus.dispose();
    scroll.dispose();
    super.dispose();
  }

  void move(int next) {
    if (!enabled || next < 0 || next >= questions.length) return;
    setState(() {
      index = next;
      error = null;
      input.text = custom['${questions[index]['id']}'] ?? '';
    });
    if (scroll.hasClients) scroll.jumpTo(0);
    if (objects(questions[index]['options']).isEmpty) {
      inputFocus.requestFocus();
    } else {
      inputFocus.unfocus();
    }
  }

  Future<void> settle({bool cancel = false}) async {
    if (!enabled) return;
    if (!cancel) {
      final missing = questions.indexWhere(
        (q) => !answered('${q['id']}') && !skipped.contains(q['id']),
      );
      if (missing >= 0) {
        move(missing);
        setState(() => error = '请回答或跳过剩余问题');
        return;
      }
    }
    setState(() {
      busy = cancel ? 'cancel' : 'answer';
      error = null;
    });
    try {
      if (cancel) {
        await widget.controller.cancelQuestion(widget.frame);
      } else {
        await widget.controller.answer(widget.frame, {
          'sessionId': widget.frame.sessionId,
          'answer': {
            'answers': [
              for (final q in questions)
                {
                  'id': q['id'],
                  'selected': skipped.contains(q['id'])
                      ? <String>[]
                      : ((custom[q['id']]?.trim().isNotEmpty ?? false) &&
                                q['multiSelect'] != true
                            ? <String>[]
                            : selected[q['id']]?.toList() ?? <String>[]),
                  if (!skipped.contains(q['id']) &&
                      (custom[q['id']]?.trim().isNotEmpty ?? false))
                    'custom': custom[q['id']]!.trim(),
                },
            ],
          },
        });
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          busy = null;
          error = '$e';
        });
      }
    }
  }

  void advance() {
    if (!enabled || !answered('${questions[index]['id']}')) return;
    if (index + 1 < questions.length) {
      move(index + 1);
    } else {
      settle();
    }
  }

  void skip() {
    if (!enabled) return;
    final id = '${questions[index]['id']}';
    selected.remove(id);
    custom.remove(id);
    skipped.add(id);
    input.clear();
    if (index + 1 < questions.length) {
      move(index + 1);
    } else {
      settle();
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (_, _) => card(context),
  );

  Widget card(BuildContext context) {
    if (questions.isEmpty) return const SizedBox.shrink();
    final q = questions[index], id = '${questions[index]['id']}';
    final options = objects(q['options']), multiple = q['multiSelect'] == true;
    final colors = DshColors(context),
        narrow = MediaQuery.sizeOf(context).width <= 720;
    return Container(
      key: const Key('question-card'),
      constraints: BoxConstraints(
        maxHeight: (MediaQuery.sizeOf(context).height * .6).clamp(0, 520),
      ),
      decoration: BoxDecoration(
        color: colors.dark ? const Color(0xff2c2c2e) : Colors.white,
        border: Border.all(color: colors.border),
        borderRadius: BorderRadius.circular(narrow ? 16 : 20),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: .05),
            blurRadius: 10,
            offset: const Offset(0, 2),
          ),
        ],
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: EdgeInsets.fromLTRB(
              narrow ? 18 : 24,
              narrow ? 10 : 20,
              16,
              0,
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          DshGlyph(
                            LucideIcons.circleHelp,
                            size: 14,
                            color: colors.muted,
                          ),
                          const SizedBox(width: 6),
                          Text(
                            busy == 'answer'
                                ? '正在提交'
                                : busy == 'cancel'
                                ? '正在放弃'
                                : '等待回答',
                            style: TextStyle(
                              fontSize: 12,
                              height: 1.5,
                              color: colors.muted,
                            ),
                          ),
                        ],
                      ),
                      if (q['header'] is String)
                        Text(
                          q['header'] as String,
                          style: TextStyle(fontSize: 11, color: colors.muted),
                        ),
                      Text(
                        '${q['question']}',
                        style: TextStyle(
                          fontSize: narrow ? 15 : 16,
                          height: 22 / 16,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ],
                  ),
                ),
                DshIcon(
                  LucideIcons.x,
                  label: '放弃整组问题',
                  size: 24,
                  onPressed: enabled ? () => settle(cancel: true) : null,
                ),
              ],
            ),
          ),
          Flexible(
            child: SingleChildScrollView(
              controller: scroll,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(12, 12, 12, 0),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (q['detail'] is String)
                      Padding(
                        padding: const EdgeInsets.fromLTRB(2, 0, 2, 8),
                        child: DshMarkdown(data: q['detail'] as String),
                      ),
                    for (final (optionIndex, option) in options.indexed)
                      optionRow(option, optionIndex, id, multiple, colors),
                    Focus(
                      onKeyEvent: (_, event) {
                        if (event is KeyDownEvent &&
                            event.logicalKey == LogicalKeyboardKey.enter &&
                            !HardwareKeyboard.instance.isShiftPressed &&
                            (!input.value.composing.isValid ||
                                input.value.composing.isCollapsed)) {
                          advance();
                          return KeyEventResult.handled;
                        }
                        return KeyEventResult.ignored;
                      },
                      child: Padding(
                        padding: options.isEmpty
                            ? const EdgeInsets.symmetric(horizontal: 12)
                            : EdgeInsets.zero,
                        child: TextField(
                          key: ValueKey('answer-$id'),
                          controller: input,
                          focusNode: inputFocus,
                          enabled: enabled,
                          minLines: options.isEmpty ? 2 : 1,
                          maxLines: options.isEmpty ? 5 : 1,
                          style: const TextStyle(fontSize: 14, height: 24 / 14),
                          decoration: InputDecoration(
                            hintText: '输入你的答案',
                            hintStyle: TextStyle(
                              color: colors.muted,
                              fontSize: 14,
                            ),
                            isDense: true,
                            contentPadding: const EdgeInsets.symmetric(
                              horizontal: 12,
                              vertical: 8,
                            ),
                            prefixIcon: options.isEmpty
                                ? null
                                : Padding(
                                    padding: const EdgeInsets.all(10),
                                    child: DshGlyph(
                                      LucideIcons.pencil,
                                      size: 16,
                                      color: colors.muted,
                                    ),
                                  ),
                            prefixIconConstraints: const BoxConstraints(
                              minWidth: 36,
                              maxWidth: 36,
                            ),
                            border: options.isEmpty
                                ? OutlineInputBorder(
                                    borderRadius: BorderRadius.circular(10),
                                    borderSide: BorderSide(
                                      color: colors.border,
                                    ),
                                  )
                                : InputBorder.none,
                            enabledBorder: options.isEmpty
                                ? OutlineInputBorder(
                                    borderRadius: BorderRadius.circular(10),
                                    borderSide: BorderSide(
                                      color: colors.border,
                                    ),
                                  )
                                : InputBorder.none,
                          ),
                          onChanged: (value) => setState(() {
                            custom[id] = value;
                            skipped.remove(id);
                            error = null;
                            if (!multiple) selected.remove(id);
                          }),
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
          if (error != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(18, 8, 10, 0),
              child: Text(
                error!,
                style: TextStyle(
                  fontSize: 11,
                  color: Theme.of(context).colorScheme.error,
                ),
              ),
            ),
          Padding(
            padding: const EdgeInsets.fromLTRB(18, 12, 10, 10),
            child: Row(
              children: [
                DshIcon(
                  LucideIcons.chevronLeft,
                  label: '上一题',
                  size: 24,
                  onPressed: enabled && index > 0
                      ? () => move(index - 1)
                      : null,
                ),
                Text(
                  '${index + 1} / ${questions.length}',
                  style: TextStyle(fontSize: 14, color: colors.muted),
                ),
                DshIcon(
                  LucideIcons.chevronRight,
                  label: '下一题',
                  size: 24,
                  onPressed: enabled && index < questions.length - 1
                      ? () => move(index + 1)
                      : null,
                ),
                const Spacer(),
                DshButton(
                  height: 36,
                  outline: true,
                  onPressed: enabled ? skip : null,
                  child: const Text('跳过本题'),
                ),
                const SizedBox(width: 12),
                DshButton(
                  key: const Key('question-continue'),
                  height: 36,
                  primary: true,
                  onPressed: enabled && answered(id) ? advance : null,
                  child: Text(
                    busy == 'answer'
                        ? '正在提交'
                        : index == questions.length - 1
                        ? '提交'
                        : '下一题',
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget optionRow(
    Json option,
    int number,
    String id,
    bool multiple,
    DshColors colors,
  ) {
    final label = '${option['label']}',
        chosen = selected[id]?.contains(option['label']) ?? false;
    final suffix = RegExp(
      r'\s*(?:\((?:recommended|推荐)\)|（(?:recommended|推荐)）)\s*$',
      caseSensitive: false,
    );
    final recommended = suffix.hasMatch(label),
        display = label.replaceFirst(suffix, '');
    return Semantics(
      checked: chosen,
      inMutuallyExclusiveGroup: !multiple,
      label: display,
      child: Material(
        color: chosen && !multiple ? colors.layer : Colors.transparent,
        borderRadius: BorderRadius.circular(12),
        child: InkWell(
          borderRadius: BorderRadius.circular(12),
          onTap: !enabled
              ? null
              : () {
                  setState(() {
                    final values = selected.putIfAbsent(id, () => {});
                    if (multiple) {
                      if (!values.add(label)) values.remove(label);
                    } else {
                      values
                        ..clear()
                        ..add(label);
                      custom.remove(id);
                      input.clear();
                    }
                    skipped.remove(id);
                    error = null;
                  });
                  if (!multiple && index < questions.length - 1) {
                    move(index + 1);
                  }
                },
          child: Padding(
            padding: const EdgeInsets.fromLTRB(8, 8, 12, 8),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SizedBox(
                  width: 20,
                  height: 24,
                  child: multiple
                      ? DshGlyph(
                          chosen ? LucideIcons.squareCheck : LucideIcons.square,
                          size: 16,
                        )
                      : Center(
                          child: Container(
                            width: 20,
                            height: 20,
                            decoration: BoxDecoration(
                              color: colors.layer,
                              borderRadius: BorderRadius.circular(6),
                            ),
                            alignment: Alignment.center,
                            child: Text(
                              '${number + 1}',
                              style: const TextStyle(fontSize: 12),
                            ),
                          ),
                        ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Wrap(
                    crossAxisAlignment: WrapCrossAlignment.center,
                    spacing: 6,
                    children: [
                      Text(
                        display,
                        style: const TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w500,
                          height: 24 / 14,
                        ),
                      ),
                      if (recommended)
                        Container(
                          padding: const EdgeInsets.symmetric(horizontal: 4),
                          decoration: BoxDecoration(
                            color: colors.blue.withValues(alpha: .1),
                            borderRadius: BorderRadius.circular(6),
                          ),
                          child: Text(
                            '推荐',
                            style: TextStyle(fontSize: 11, color: colors.blue),
                          ),
                        ),
                      if (option['description'] is String)
                        Text(
                          option['description'] as String,
                          style: TextStyle(
                            fontSize: 14,
                            height: 24 / 14,
                            color: colors.muted,
                          ),
                        ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
