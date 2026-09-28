; Shared one-screen installer surface for the Web core and desktop installers:
; a white page with the round whale logo, the wordmark, a "choose location"
; link that reveals the folder and shortcut options, and one black primary
; button; installation progress and completion stay on the same surface.
;
; The including script defines, before this file:
;   ArtDir        folder holding the generated bitmaps (see build-art.ps1)
;   DshTitle      window title
;   DshAppSubdir  folder appended to a location picked with Browse
; and its NextButtonClick calls DshIsLanding / DshApplyChoices / DshShowError.
; Only APIs available in Inno Setup 6.1.2 (the CI compiler) are used.

[Files]
Source: "{#ArtDir}\logo-1x.bmp"; Flags: dontcopy
Source: "{#ArtDir}\logo-2x.bmp"; Flags: dontcopy
Source: "{#ArtDir}\wordmark-1x.bmp"; Flags: dontcopy
Source: "{#ArtDir}\wordmark-2x.bmp"; Flags: dontcopy
Source: "{#ArtDir}\button-1x.bmp"; Flags: dontcopy
Source: "{#ArtDir}\button-2x.bmp"; Flags: dontcopy

[CustomMessages]
english.DshInstallNow=Install now
english.DshChooseLocation=Choose install location
english.DshHideLocation=Hide install location
english.DshBrowse=Browse…
english.DshBrowsePrompt=Choose where to install. A folder for the application is created inside it.
english.DshInstalling=Installing…
english.DshFinishing=Finishing setup…
english.DshInstalled=Installation complete
english.DshLaunchNow=Start now
english.DshClose=Close
chinesesimp.DshInstallNow=立即安装
chinesesimp.DshChooseLocation=选择安装位置
chinesesimp.DshHideLocation=收起安装位置
chinesesimp.DshBrowse=浏览…
chinesesimp.DshBrowsePrompt=选择安装位置，程序会安装到其中的独立文件夹。
chinesesimp.DshInstalling=正在安装…
chinesesimp.DshFinishing=正在完成配置…
chinesesimp.DshInstalled=安装完成
chinesesimp.DshLaunchNow=立即体验
chinesesimp.DshClose=完成

[Code]
const
  DshModeLanding = 0;
  DshModeInstalling = 1;
  DshModeDone = 2;
  DshWidth = 520;
  DshHeight = 440;
  DshTextColor = $00262626;
  DshMutedColor = $00988F8A;
  DshErrorColor = $002530D9;

var
  DshPage: TWizardPage;
  DshSurface: TPanel;
  DshLogo, DshWordmark, DshButtonImage: TBitmapImage;
  DshButtonLabel, DshLinkLabel, DshStatusLabel, DshErrorLabel, DshCloseLabel: TLabel;
  DshPathEdit: TNewEdit;
  DshBrowseButton: TNewButton;
  DshShortcut: TNewCheckBox;
  DshProgress: TNewProgressBar;
  DshExpanded, DshPathLoaded: Boolean;
  DshMode: Integer;

function DshIsLanding(PageID: Integer): Boolean;
begin
  Result := (DshPage <> nil) and (PageID = DshPage.ID);
end;

function DshFontName: String;
begin
  if ActiveLanguage = 'chinesesimp' then
    Result := 'Microsoft YaHei UI'
  else
    Result := 'Segoe UI';
end;

function DshArt(Name: String): String;
var
  Scale: String;
begin
  { Pick the sharper source for this DPI; the control stretches it. }
  if ScaleX(100) > 125 then Scale := '2x' else Scale := '1x';
  Result := Name + '-' + Scale + '.bmp';
  ExtractTemporaryFile(Result);
  Result := ExpandConstant('{tmp}\') + Result;
end;

function DshImage(Name: String; Width, Height: Integer): TBitmapImage;
begin
  Result := TBitmapImage.Create(DshSurface);
  Result.Parent := DshSurface;
  Result.Stretch := True;
  Result.Bitmap.LoadFromFile(DshArt(Name));
  Result.SetBounds(ScaleX((DshWidth - Width) div 2), 0, ScaleX(Width), ScaleY(Height));
end;

function DshLabel(Caption: String; Size: Integer; Color: Integer): TLabel;
begin
  Result := TLabel.Create(DshSurface);
  Result.Parent := DshSurface;
  Result.AutoSize := False;
  Result.Alignment := taCenter;
  Result.Transparent := True;
  Result.Font.Name := DshFontName;
  Result.Font.Size := Size;
  Result.Font.Color := Color;
  Result.Caption := Caption;
  Result.SetBounds(ScaleX(40), 0, ScaleX(DshWidth - 80), ScaleY(24));
end;

procedure DshPlace(Control: TControl; Top: Integer; Visible: Boolean);
begin
  Control.Top := ScaleY(Top);
  Control.Visible := Visible;
end;

{ Lay out the surface for the current mode; the landing layout moves the
  brand up when the location options are open. }
procedure DshLayout;
var
  Shift: Integer;
  Landing: Boolean;
begin
  Landing := DshMode = DshModeLanding;
  if Landing and DshExpanded then Shift := 32 else Shift := 0;
  DshPlace(DshLogo, 60 - Shift, True);
  DshPlace(DshWordmark, 200 - Shift, True);
  if DshExpanded then
    DshLinkLabel.Caption := CustomMessage('DshHideLocation')
  else
    DshLinkLabel.Caption := CustomMessage('DshChooseLocation');
  DshPlace(DshLinkLabel, 272 - Shift, Landing);
  DshPlace(DshPathEdit, 262, Landing and DshExpanded);
  DshPlace(DshBrowseButton, 261, Landing and DshExpanded);
  DshPlace(DshShortcut, 298, Landing and DshExpanded);
  DshPlace(DshErrorLabel, 322, Landing and DshExpanded and (DshErrorLabel.Caption <> ''));
  DshPlace(DshStatusLabel, 276, DshMode <> DshModeLanding);
  DshPlace(DshProgress, 308, DshMode = DshModeInstalling);
  DshPlace(DshButtonImage, 348, DshMode <> DshModeInstalling);
  DshPlace(DshButtonLabel, 348 + 12, DshMode <> DshModeInstalling);
  DshPlace(DshCloseLabel, 402, DshMode = DshModeDone);
  if Landing then
    DshButtonLabel.Caption := CustomMessage('DshInstallNow')
  else
    DshButtonLabel.Caption := CustomMessage('DshLaunchNow');
end;

procedure DshShowError(Message: String);
begin
  DshErrorLabel.Caption := Message;
  DshExpanded := True;
  DshLayout;
end;

{ Copy the surface choices into the wizard before installation starts. }
procedure DshApplyChoices;
begin
  WizardForm.DirEdit.Text := Trim(DshPathEdit.Text);
  if DshShortcut.Checked then
    WizardSelectTasks('desktopicon')
  else
    WizardSelectTasks('!desktopicon');
  DshErrorLabel.Caption := '';
end;

procedure DshPrimaryClick(Sender: TObject);
var
  Index: Integer;
begin
  if DshMode = DshModeDone then
    for Index := 0 to WizardForm.RunList.Items.Count - 1 do
      WizardForm.RunList.Checked[Index] := True;
  WizardForm.NextButton.OnClick(WizardForm.NextButton);
end;

procedure DshCloseClick(Sender: TObject);
var
  Index: Integer;
begin
  for Index := 0 to WizardForm.RunList.Items.Count - 1 do
    WizardForm.RunList.Checked[Index] := False;
  WizardForm.NextButton.OnClick(WizardForm.NextButton);
end;

procedure DshToggleLocation(Sender: TObject);
begin
  DshExpanded := not DshExpanded;
  DshLayout;
  if DshExpanded then WizardForm.ActiveControl := DshPathEdit;
end;

{ Start browsing at the nearest existing folder of the current choice, which
  is on drive D: by default. }
procedure DshBrowseClick(Sender: TObject);
var
  Directory, Picked, Suffix: String;
begin
  Directory := Trim(DshPathEdit.Text);
  while (Directory <> '') and not DirExists(Directory) and
        (ExtractFileDir(Directory) <> Directory) do
    Directory := ExtractFileDir(Directory);
  Picked := Directory;
  if BrowseForFolder(CustomMessage('DshBrowsePrompt'), Picked, True) then
  begin
    Suffix := '\{#DshAppSubdir}';
    if Lowercase(Copy(RemoveBackslash(Picked), Length(RemoveBackslash(Picked)) - Length(Suffix) + 1, Length(Suffix))) = Lowercase(Suffix) then
      DshPathEdit.Text := RemoveBackslash(Picked)
    else
      DshPathEdit.Text := RemoveBackslash(Picked) + Suffix;
    DshErrorLabel.Caption := '';
    DshLayout;
  end;
end;

procedure InitializeWizard;
begin
  DshPage := CreateCustomPage(wpWelcome, '', '');
  WizardForm.Caption := '{#DshTitle}';
  WizardForm.ClientWidth := ScaleX(DshWidth);
  WizardForm.ClientHeight := ScaleY(DshHeight);
  WizardForm.Position := poScreenCenter;
  WizardForm.BorderIcons := [biSystemMenu, biMinimize];

  DshSurface := TPanel.Create(WizardForm);
  DshSurface.Parent := WizardForm;
  DshSurface.BevelOuter := bvNone;
  DshSurface.ParentBackground := False;
  DshSurface.Color := $FFFFFF;
  DshSurface.SetBounds(0, 0, WizardForm.ClientWidth, WizardForm.ClientHeight);
  { The including scripts pin WizardSizePercent=100; the anchors still keep
    the stock wizard hidden if Setup resizes the form after this point. }
  DshSurface.Anchors := [akLeft, akTop, akRight, akBottom];
  DshSurface.BringToFront;

  DshLogo := DshImage('logo', 112, 112);
  DshWordmark := DshImage('wordmark', 208, 32);

  DshLinkLabel := DshLabel('', 9, DshMutedColor);
  DshLinkLabel.Height := ScaleY(20);
  DshLinkLabel.Cursor := crHand;
  DshLinkLabel.OnClick := @DshToggleLocation;

  DshPathEdit := TNewEdit.Create(DshSurface);
  DshPathEdit.Parent := DshSurface;
  DshPathEdit.Font.Name := DshFontName;
  DshPathEdit.SetBounds(ScaleX(70), 0, ScaleX(290), ScaleY(26));

  DshBrowseButton := TNewButton.Create(DshSurface);
  DshBrowseButton.Parent := DshSurface;
  DshBrowseButton.Caption := CustomMessage('DshBrowse');
  DshBrowseButton.SetBounds(ScaleX(368), 0, ScaleX(82), ScaleY(28));
  DshBrowseButton.OnClick := @DshBrowseClick;

  DshShortcut := TNewCheckBox.Create(DshSurface);
  DshShortcut.Parent := DshSurface;
  DshShortcut.Caption := CustomMessage('DesktopShortcut');
  DshShortcut.Font.Name := DshFontName;
  DshShortcut.Font.Color := DshTextColor;
  DshShortcut.SetBounds(ScaleX(70), 0, ScaleX(380), ScaleY(20));

  DshErrorLabel := DshLabel('', 8, DshErrorColor);
  DshErrorLabel.WordWrap := True;
  DshErrorLabel.SetBounds(ScaleX(60), 0, ScaleX(DshWidth - 120), ScaleY(24));

  DshStatusLabel := DshLabel('', 10, DshTextColor);

  DshProgress := TNewProgressBar.Create(DshSurface);
  DshProgress.Parent := DshSurface;
  DshProgress.SetBounds(ScaleX(110), 0, ScaleX(DshWidth - 220), ScaleY(6));
  DshProgress.Min := 0;
  DshProgress.Max := 100;

  DshButtonImage := DshImage('button', 220, 44);
  DshButtonImage.Cursor := crHand;
  DshButtonImage.OnClick := @DshPrimaryClick;
  DshButtonLabel := DshLabel('', 10, $FFFFFF);
  DshButtonLabel.SetBounds(DshButtonImage.Left, 0, DshButtonImage.Width, ScaleY(22));
  DshButtonLabel.Cursor := crHand;
  DshButtonLabel.OnClick := @DshPrimaryClick;

  DshCloseLabel := DshLabel(CustomMessage('DshClose'), 9, DshMutedColor);
  DshCloseLabel.Cursor := crHand;
  DshCloseLabel.OnClick := @DshCloseClick;

  DshMode := DshModeLanding;
  DshLayout;
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  { The surface replaces the folder, program group, task and ready pages. }
  Result := (PageID = wpSelectDir) or (PageID = wpSelectProgramGroup) or
    (PageID = wpSelectTasks) or (PageID = wpReady);
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if DshIsLanding(CurPageID) then
  begin
    if not DshPathLoaded then
    begin
      DshPathEdit.Text := WizardForm.DirEdit.Text;
      DshShortcut.Checked := WizardIsTaskSelected('desktopicon');
      DshPathLoaded := True;
    end;
    DshMode := DshModeLanding;
  end
  else if CurPageID = wpInstalling then
  begin
    DshMode := DshModeInstalling;
    DshStatusLabel.Caption := CustomMessage('DshInstalling') + ' 0%';
  end
  else if CurPageID = wpFinished then
  begin
    DshMode := DshModeDone;
    DshStatusLabel.Caption := CustomMessage('DshInstalled');
  end;
  { Pages that report a problem (preparing errors) show the standard wizard. }
  DshSurface.Visible := DshIsLanding(CurPageID) or (CurPageID = wpInstalling) or
    (CurPageID = wpFinished);
  if DshSurface.Visible then
  begin
    DshSurface.BringToFront;
    DshLayout;
  end;
end;

procedure CurInstallProgressChanged(CurProgress, MaxProgress: Integer);
var
  Percent: Integer;
begin
  if MaxProgress <= 0 then Exit;
  Percent := (CurProgress * 100) div MaxProgress;
  DshProgress.Position := Percent;
  if Percent >= 100 then
    DshStatusLabel.Caption := CustomMessage('DshFinishing')
  else
    DshStatusLabel.Caption := CustomMessage('DshInstalling') + ' ' + IntToStr(Percent) + '%';
end;
