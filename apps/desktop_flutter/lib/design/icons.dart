import 'package:flutter/widgets.dart';

/// Semantic vector identities. The local IDs are opaque compatibility values;
/// they never select a glyph from a third-party icon font.
enum DshIcons {
  alarmClock(
    'assets/icons/alarmClock.svg',
    IconData(0xe000, fontFamily: 'DshVectorIcons'),
  ),
  archive(
    'assets/icons/archive.svg',
    IconData(0xe001, fontFamily: 'DshVectorIcons'),
  ),
  arrowDown(
    'assets/icons/arrowDown.svg',
    IconData(0xe002, fontFamily: 'DshVectorIcons'),
  ),
  arrowLeft(
    'assets/icons/arrowLeft.svg',
    IconData(0xe003, fontFamily: 'DshVectorIcons'),
  ),
  arrowUp(
    'assets/icons/web-IconSendOutline16.svg',
    IconData(0xe004, fontFamily: 'DshVectorIcons'),
  ),
  bookOpen(
    'assets/icons/bookOpen.svg',
    IconData(0xe005, fontFamily: 'DshVectorIcons'),
  ),
  brain(
    'assets/icons/brain.svg',
    IconData(0xe006, fontFamily: 'DshVectorIcons'),
  ),
  briefcase(
    'assets/icons/web-IconSkillOutline16.svg',
    IconData(0xe007, fontFamily: 'DshVectorIcons'),
  ),
  calendarClock(
    'assets/icons/desktop-schedule.svg',
    IconData(0xe008, fontFamily: 'DshVectorIcons'),
  ),
  check(
    'assets/icons/web-IconCheckOutline16.svg',
    IconData(0xe009, fontFamily: 'DshVectorIcons'),
  ),
  chevronDown(
    'assets/icons/web-IconChevronDownOutline14.svg',
    IconData(0xe00b, fontFamily: 'DshVectorIcons'),
  ),
  chevronLeft(
    'assets/icons/web-IconChevronLeftOutline14.svg',
    IconData(0xe00c, fontFamily: 'DshVectorIcons'),
  ),
  chevronRight(
    'assets/icons/web-IconChevronRightOutline14.svg',
    IconData(0xe00d, fontFamily: 'DshVectorIcons'),
  ),
  chevronUp(
    'assets/icons/web-IconChevronUpOutline14.svg',
    IconData(0xe00e, fontFamily: 'DshVectorIcons'),
  ),
  circle(
    'assets/icons/circle.svg',
    IconData(0xe00f, fontFamily: 'DshVectorIcons'),
  ),
  circleAlert(
    'assets/icons/web-IconWarningOutline16.svg',
    IconData(0xe010, fontFamily: 'DshVectorIcons'),
  ),
  circleCheck(
    'assets/icons/circleCheck.svg',
    IconData(0xe011, fontFamily: 'DshVectorIcons'),
  ),
  circleHelp(
    'assets/icons/web-IconQuestionOutline14.svg',
    IconData(0xe012, fontFamily: 'DshVectorIcons'),
  ),
  circlePlus(
    'assets/icons/web-IconNewChatOutline16.svg',
    IconData(0xe013, fontFamily: 'DshVectorIcons'),
  ),
  clock(
    'assets/icons/desktop-clock.svg',
    IconData(0xe014, fontFamily: 'DshVectorIcons'),
  ),
  clock3(
    'assets/icons/clock3.svg',
    IconData(0xe015, fontFamily: 'DshVectorIcons'),
  ),
  code(
    'assets/icons/web-IconCodeOutline16.svg',
    IconData(0xe016, fontFamily: 'DshVectorIcons'),
  ),
  copy(
    'assets/icons/web-IconCopyOutline16.svg',
    IconData(0xe017, fontFamily: 'DshVectorIcons'),
  ),
  database(
    'assets/icons/web-IconDataOutline16.svg',
    IconData(0xe018, fontFamily: 'DshVectorIcons'),
  ),
  download(
    'assets/icons/web-IconDownloadOutline16.svg',
    IconData(0xe019, fontFamily: 'DshVectorIcons'),
  ),
  ellipsis(
    'assets/icons/web-IconEllipsisOutline16.svg',
    IconData(0xe01a, fontFamily: 'DshVectorIcons'),
  ),
  eye('assets/icons/eye.svg', IconData(0xe01b, fontFamily: 'DshVectorIcons')),
  file('assets/icons/file.svg', IconData(0xe01c, fontFamily: 'DshVectorIcons')),
  filePlus(
    'assets/icons/filePlus.svg',
    IconData(0xe01d, fontFamily: 'DshVectorIcons'),
  ),
  fileText(
    'assets/icons/fileText.svg',
    IconData(0xe01e, fontFamily: 'DshVectorIcons'),
  ),
  files(
    'assets/icons/desktop-files.svg',
    IconData(0xe01f, fontFamily: 'DshVectorIcons'),
  ),
  folder(
    'assets/icons/desktop-folder.svg',
    IconData(0xe020, fontFamily: 'DshVectorIcons'),
  ),
  folderCog(
    'assets/icons/folderCog.svg',
    IconData(0xe021, fontFamily: 'DshVectorIcons'),
  ),
  folderOpen(
    'assets/icons/web-IconFolderOpenOutline16.svg',
    IconData(0xe022, fontFamily: 'DshVectorIcons'),
  ),
  folderPlus(
    'assets/icons/web-IconProjectAddOutline16.svg',
    IconData(0xe023, fontFamily: 'DshVectorIcons'),
  ),
  gitBranch(
    'assets/icons/web-IconBranchOutline16.svg',
    IconData(0xe024, fontFamily: 'DshVectorIcons'),
  ),
  gitCompareArrows(
    'assets/icons/gitCompareArrows.svg',
    IconData(0xe025, fontFamily: 'DshVectorIcons'),
  ),
  grid2x2(
    'assets/icons/grid2x2.svg',
    IconData(0xe026, fontFamily: 'DshVectorIcons'),
  ),
  history(
    'assets/icons/history.svg',
    IconData(0xe027, fontFamily: 'DshVectorIcons'),
  ),
  image(
    'assets/icons/image.svg',
    IconData(0xe028, fontFamily: 'DshVectorIcons'),
  ),
  inbox(
    'assets/icons/inbox.svg',
    IconData(0xe029, fontFamily: 'DshVectorIcons'),
  ),
  keyboard(
    'assets/icons/keyboard.svg',
    IconData(0xe02a, fontFamily: 'DshVectorIcons'),
  ),
  layoutGrid(
    'assets/icons/desktop-grid.svg',
    IconData(0xe02b, fontFamily: 'DshVectorIcons'),
  ),
  link(
    'assets/icons/web-IconLinkOutline16.svg',
    IconData(0xe02c, fontFamily: 'DshVectorIcons'),
  ),
  listChecks(
    'assets/icons/desktop-list-checks.svg',
    IconData(0xe02d, fontFamily: 'DshVectorIcons'),
  ),
  listFilter(
    'assets/icons/listFilter.svg',
    IconData(0xe02e, fontFamily: 'DshVectorIcons'),
  ),
  loaderCircle(
    'assets/icons/web-IconLoadingOutline16.svg',
    IconData(0xe02f, fontFamily: 'DshVectorIcons'),
  ),
  maximize(
    'assets/icons/maximize.svg',
    IconData(0xe030, fontFamily: 'DshVectorIcons'),
  ),
  messageCircle(
    'assets/icons/messageCircle.svg',
    IconData(0xe031, fontFamily: 'DshVectorIcons'),
  ),
  mic(
    'assets/icons/desktop-mic.svg',
    IconData(0xe032, fontFamily: 'DshVectorIcons'),
  ),
  minus(
    'assets/icons/minus.svg',
    IconData(0xe033, fontFamily: 'DshVectorIcons'),
  ),
  moon(
    'assets/icons/web-IconDarkOutline16.svg',
    IconData(0xe034, fontFamily: 'DshVectorIcons'),
  ),
  panelLeft(
    'assets/icons/web-IconPanelLeftOutline16.svg',
    IconData(0xe035, fontFamily: 'DshVectorIcons'),
  ),
  panelLeftClose(
    'assets/icons/web-IconPanelLeftOutline16.svg',
    IconData(0xe036, fontFamily: 'DshVectorIcons'),
  ),
  panelRight(
    'assets/icons/desktop-panel-right.svg',
    IconData(0xe037, fontFamily: 'DshVectorIcons'),
  ),
  paperclip(
    'assets/icons/web-IconPaperclipOutline16.svg',
    IconData(0xe038, fontFamily: 'DshVectorIcons'),
  ),
  pause(
    'assets/icons/web-IconPauseOutline16.svg',
    IconData(0xe039, fontFamily: 'DshVectorIcons'),
  ),
  pencil(
    'assets/icons/web-IconEditOutline16.svg',
    IconData(0xe03a, fontFamily: 'DshVectorIcons'),
  ),
  play(
    'assets/icons/web-IconPlayOutline16.svg',
    IconData(0xe03b, fontFamily: 'DshVectorIcons'),
  ),
  plus(
    'assets/icons/web-IconPlusOutline16.svg',
    IconData(0xe03c, fontFamily: 'DshVectorIcons'),
  ),
  puzzle(
    'assets/icons/puzzle.svg',
    IconData(0xe03d, fontFamily: 'DshVectorIcons'),
  ),
  refreshCw(
    'assets/icons/web-IconRefreshOutline16.svg',
    IconData(0xe03e, fontFamily: 'DshVectorIcons'),
  ),
  rotateCcw(
    'assets/icons/rotateCcw.svg',
    IconData(0xe03f, fontFamily: 'DshVectorIcons'),
  ),
  rotateCw(
    'assets/icons/web-IconRefreshOutline16.svg',
    IconData(0xe040, fontFamily: 'DshVectorIcons'),
  ),
  search(
    'assets/icons/web-IconSearchOutline16.svg',
    IconData(0xe041, fontFamily: 'DshVectorIcons'),
  ),
  searchX(
    'assets/icons/searchX.svg',
    IconData(0xe042, fontFamily: 'DshVectorIcons'),
  ),
  settings(
    'assets/icons/web-IconSettingsOutline16.svg',
    IconData(0xe043, fontFamily: 'DshVectorIcons'),
  ),
  shield(
    'assets/icons/shield.svg',
    IconData(0xe044, fontFamily: 'DshVectorIcons'),
  ),
  shieldAlert(
    'assets/icons/shieldAlert.svg',
    IconData(0xe045, fontFamily: 'DshVectorIcons'),
  ),
  shieldCheck(
    'assets/icons/desktop-shield-check.svg',
    IconData(0xe046, fontFamily: 'DshVectorIcons'),
  ),
  slidersHorizontal(
    'assets/icons/slidersHorizontal.svg',
    IconData(0xe047, fontFamily: 'DshVectorIcons'),
  ),
  square(
    'assets/icons/web-IconStopFill16.svg',
    IconData(0xe048, fontFamily: 'DshVectorIcons'),
  ),
  squareCheck(
    'assets/icons/squareCheck.svg',
    IconData(0xe049, fontFamily: 'DshVectorIcons'),
  ),
  squareTerminal(
    'assets/icons/squareTerminal.svg',
    IconData(0xe04a, fontFamily: 'DshVectorIcons'),
  ),
  sun(
    'assets/icons/web-IconLightOutline16.svg',
    IconData(0xe04b, fontFamily: 'DshVectorIcons'),
  ),
  terminal(
    'assets/icons/desktop-terminal.svg',
    IconData(0xe04c, fontFamily: 'DshVectorIcons'),
  ),
  thumbsDown(
    'assets/icons/web-IconDislikeOutline16.svg',
    IconData(0xe04d, fontFamily: 'DshVectorIcons'),
  ),
  thumbsUp(
    'assets/icons/web-IconLikeOutline16.svg',
    IconData(0xe04e, fontFamily: 'DshVectorIcons'),
  ),
  trash2(
    'assets/icons/web-IconTrashOutline16.svg',
    IconData(0xe04f, fontFamily: 'DshVectorIcons'),
  ),
  unplug(
    'assets/icons/unplug.svg',
    IconData(0xe050, fontFamily: 'DshVectorIcons'),
  ),
  users(
    'assets/icons/users.svg',
    IconData(0xe051, fontFamily: 'DshVectorIcons'),
  ),
  video(
    'assets/icons/video.svg',
    IconData(0xe052, fontFamily: 'DshVectorIcons'),
  ),
  volume2(
    'assets/icons/volume2.svg',
    IconData(0xe053, fontFamily: 'DshVectorIcons'),
  ),
  workflow(
    'assets/icons/web-IconAgentPresetOutline16.svg',
    IconData(0xe054, fontFamily: 'DshVectorIcons'),
  ),
  wrapText(
    'assets/icons/wrapText.svg',
    IconData(0xe055, fontFamily: 'DshVectorIcons'),
  ),
  wrench(
    'assets/icons/wrench.svg',
    IconData(0xe056, fontFamily: 'DshVectorIcons'),
  ),
  x(
    'assets/icons/web-IconCloseOutline16.svg',
    IconData(0xe057, fontFamily: 'DshVectorIcons'),
  ),
  send(
    'assets/icons/web-IconSendOutline16.svg',
    IconData(0xe058, fontFamily: 'DshVectorIcons'),
  ),
  stop(
    'assets/icons/web-IconStopFill16.svg',
    IconData(0xe059, fontFamily: 'DshVectorIcons'),
  ),
  attach(
    'assets/icons/web-IconPaperclipOutline16.svg',
    IconData(0xe05a, fontFamily: 'DshVectorIcons'),
  ),
  approval(
    'assets/icons/shieldCheck.svg',
    IconData(0xe05b, fontFamily: 'DshVectorIcons'),
  ),
  warning(
    'assets/icons/web-IconWarningOutline16.svg',
    IconData(0xe05c, fontFamily: 'DshVectorIcons'),
  ),
  success(
    'assets/icons/circleCheck.svg',
    IconData(0xe05d, fontFamily: 'DshVectorIcons'),
  ),
  failure(
    'assets/icons/failure.svg',
    IconData(0xe05e, fontFamily: 'DshVectorIcons'),
  ),
  expand(
    'assets/icons/web-IconChevronDownOutline14.svg',
    IconData(0xe05f, fontFamily: 'DshVectorIcons'),
  ),
  collapse(
    'assets/icons/web-IconChevronUpOutline14.svg',
    IconData(0xe060, fontFamily: 'DshVectorIcons'),
  ),
  refresh(
    'assets/icons/web-IconRefreshOutline16.svg',
    IconData(0xe061, fontFamily: 'DshVectorIcons'),
  ),
  retry(
    'assets/icons/web-IconRefreshOutline16.svg',
    IconData(0xe062, fontFamily: 'DshVectorIcons'),
  ),
  newSession(
    'assets/icons/web-IconNewChatOutline16.svg',
    IconData(0xe063, fontFamily: 'DshVectorIcons'),
  ),
  conversation(
    'assets/icons/messageCircle.svg',
    IconData(0xe064, fontFamily: 'DshVectorIcons'),
  ),
  close(
    'assets/icons/web-IconCloseOutline16.svg',
    IconData(0xe065, fontFamily: 'DshVectorIcons'),
  ),
  unknown(
    'assets/icons/web-IconQuestionOutline14.svg',
    IconData(0xe066, fontFamily: 'DshVectorIcons'),
  ),
  checkboxUnchecked(
    'assets/icons/checkbox-unchecked.svg',
    IconData(0xe067, fontFamily: 'DshVectorIcons'),
  ),
  browser(
    'assets/icons/desktop-browser.svg',
    IconData(0xe068, fontFamily: 'DshVectorIcons'),
  ),
  plugins(
    'assets/icons/desktop-plugins.svg',
    IconData(0xe069, fontFamily: 'DshVectorIcons'),
  );

  const DshIcons(this.asset, this.data);

  final String asset;
  final IconData data;

  static final Map<IconData, DshIcons> _identities = {
    for (final icon in values) icon.data: icon,
  };

  static String? assetFor(IconData? data) {
    if (data == null) return null;
    if (data.fontFamily != 'DshVectorIcons') return null;
    return (_identities[data] ?? unknown).asset;
  }
}
