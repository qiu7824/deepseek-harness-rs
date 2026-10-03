/// Lightweight position only; history bodies remain in the bounded Host window.
class ConversationReadingPosition {
  const ConversationReadingPosition({
    required this.seq,
    required this.itemId,
    required this.viewportOffset,
  });
  final int seq;
  final String itemId;

  /// The message's top edge relative to the viewport, including a negative
  /// offset when the reader is part way through a long message.
  final double viewportOffset;
}
