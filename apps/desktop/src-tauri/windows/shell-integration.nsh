; Optional, per-user Windows shell integration for Code Hangar.
; This never writes Windows' protected UserChoice keys. The installer only
; registers Code Hangar as an available handler and the app guides the user to
; Default Apps when they explicitly opt in.

!ifndef CODEHANGAR_PINNED_WEBVIEW2_READY
  !error "Code Hangar installers require the canonical pinned-WebView2 packaging hook."
!endif

Var CodeHangarMarkdownChoice
Var CodeHangarContextChoice
Var CodeHangarMarkdownWasEnabled
Var CodeHangarContextWasEnabled
Var CodeHangarShellExecutable

!macro CODEHANGAR_NOTIFY_SHELL
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0x0000, p 0, p 0)'
!macroend

!macro CODEHANGAR_REGISTER_MARKDOWN
  WriteRegStr HKCU "Software\RegisteredApplications" "Code Hangar" "Software\JCOM Labs\Code Hangar\Capabilities"
  WriteRegStr HKCU "Software\JCOM Labs\Code Hangar\Capabilities" "ApplicationName" "Code Hangar"
  WriteRegStr HKCU "Software\JCOM Labs\Code Hangar\Capabilities" "ApplicationDescription" "Local-first project and Markdown explorer"
  WriteRegStr HKCU "Software\JCOM Labs\Code Hangar\Capabilities\FileAssociations" ".md" "CodeHangar.Markdown"
  WriteRegStr HKCU "Software\JCOM Labs\Code Hangar\Capabilities\FileAssociations" ".markdown" "CodeHangar.Markdown"
  WriteRegStr HKCU "Software\JCOM Labs\Code Hangar\Capabilities\FileAssociations" ".mdx" "CodeHangar.Markdown"

  WriteRegStr HKCU "Software\Classes\.md\OpenWithProgids" "CodeHangar.Markdown" ""
  WriteRegStr HKCU "Software\Classes\.markdown\OpenWithProgids" "CodeHangar.Markdown" ""
  WriteRegStr HKCU "Software\Classes\.mdx\OpenWithProgids" "CodeHangar.Markdown" ""
  WriteRegStr HKCU "Software\Classes\CodeHangar.Markdown" "" "Markdown document"
  WriteRegStr HKCU "Software\Classes\CodeHangar.Markdown" "FriendlyTypeName" "Markdown document"
  WriteRegStr HKCU "Software\Classes\CodeHangar.Markdown\DefaultIcon" "" "$\"$CodeHangarShellExecutable$\",0"
  WriteRegStr HKCU "Software\Classes\CodeHangar.Markdown\shell\open\command" "" "$\"$CodeHangarShellExecutable$\" $\"%1$\""
!macroend

!macro CODEHANGAR_UNREGISTER_MARKDOWN
  DeleteRegKey HKCU "Software\Classes\CodeHangar.Markdown"
  DeleteRegKey HKCU "Software\JCOM Labs\Code Hangar\Capabilities"
  DeleteRegValue HKCU "Software\RegisteredApplications" "Code Hangar"
  DeleteRegValue HKCU "Software\Classes\.md\OpenWithProgids" "CodeHangar.Markdown"
  DeleteRegValue HKCU "Software\Classes\.markdown\OpenWithProgids" "CodeHangar.Markdown"
  DeleteRegValue HKCU "Software\Classes\.mdx\OpenWithProgids" "CodeHangar.Markdown"
!macroend

!macro CODEHANGAR_REGISTER_CONTEXT_MENU
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\text\shell\CodeHangar" "" "Open in Code Hangar"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\text\shell\CodeHangar" "Icon" "$\"$CodeHangarShellExecutable$\",0"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\text\shell\CodeHangar\command" "" "$\"$CodeHangarShellExecutable$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.md\shell\CodeHangar" "" "Open in Code Hangar"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.md\shell\CodeHangar" "Icon" "$\"$CodeHangarShellExecutable$\",0"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.md\shell\CodeHangar\command" "" "$\"$CodeHangarShellExecutable$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.markdown\shell\CodeHangar" "" "Open in Code Hangar"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.markdown\shell\CodeHangar" "Icon" "$\"$CodeHangarShellExecutable$\",0"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.markdown\shell\CodeHangar\command" "" "$\"$CodeHangarShellExecutable$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.mdx\shell\CodeHangar" "" "Open in Code Hangar"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.mdx\shell\CodeHangar" "Icon" "$\"$CodeHangarShellExecutable$\",0"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.mdx\shell\CodeHangar\command" "" "$\"$CodeHangarShellExecutable$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\Directory\shell\CodeHangar" "" "Open folder in Code Hangar"
  WriteRegStr HKCU "Software\Classes\Directory\shell\CodeHangar" "Icon" "$\"$CodeHangarShellExecutable$\",0"
  WriteRegStr HKCU "Software\Classes\Directory\shell\CodeHangar\command" "" "$\"$CodeHangarShellExecutable$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\CodeHangar" "" "Open folder in Code Hangar"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\CodeHangar" "Icon" "$\"$CodeHangarShellExecutable$\",0"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\CodeHangar\command" "" "$\"$CodeHangarShellExecutable$\" $\"%V$\""
!macroend

!macro CODEHANGAR_UNREGISTER_CONTEXT_MENU
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\text\shell\CodeHangar"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.md\shell\CodeHangar"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.markdown\shell\CodeHangar"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.mdx\shell\CodeHangar"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\CodeHangar"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\CodeHangar"
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro CODEHANGAR_INSTALL_PINNED_WEBVIEW2
  StrCpy $CodeHangarMarkdownChoice 0
  StrCpy $CodeHangarContextChoice 0
  StrCpy $CodeHangarMarkdownWasEnabled 0
  StrCpy $CodeHangarContextWasEnabled 0
  ClearErrors
  ReadRegDWORD $CodeHangarMarkdownWasEnabled HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "MarkdownEnabled"
  ClearErrors
  ReadRegDWORD $CodeHangarContextWasEnabled HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "ContextMenuEnabled"
  StrCpy $CodeHangarMarkdownChoice $CodeHangarMarkdownWasEnabled
  StrCpy $CodeHangarContextChoice $CodeHangarContextWasEnabled

  ; Silent/passive installs preserve the existing preference and never surprise
  ; the user with a new Explorer integration.
  ${If} ${Silent}
  ${OrIf} $PassiveMode = 1
    Goto codehangar_choices_done
  ${EndIf}

  ${If} $CodeHangarMarkdownWasEnabled = 1
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON1 "Register Code Hangar in Windows 'Open with' for .md, .markdown and .mdx files?$\r$\n$\r$\nWindows will keep control of the default app; Code Hangar will guide you to Default Apps after installation." IDYES codehangar_markdown_yes IDNO codehangar_markdown_no
  ${Else}
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Register Code Hangar in Windows 'Open with' for .md, .markdown and .mdx files?$\r$\n$\r$\nWindows will keep control of the default app; Code Hangar will guide you to Default Apps after installation." IDYES codehangar_markdown_yes IDNO codehangar_markdown_no
  ${EndIf}
  codehangar_markdown_yes:
    StrCpy $CodeHangarMarkdownChoice 1
    Goto codehangar_context_question
  codehangar_markdown_no:
    StrCpy $CodeHangarMarkdownChoice 0

  codehangar_context_question:
  ${If} $CodeHangarContextWasEnabled = 1
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON1 "Add 'Open in Code Hangar' to File Explorer for text files and folders?$\r$\n$\r$\nThis per-user menu is optional and can be removed later in Code Hangar Settings." IDYES codehangar_context_yes IDNO codehangar_context_no
  ${Else}
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Add 'Open in Code Hangar' to File Explorer for text files and folders?$\r$\n$\r$\nThis per-user menu is optional and can be removed later in Code Hangar Settings." IDYES codehangar_context_yes IDNO codehangar_context_no
  ${EndIf}
  codehangar_context_yes:
    StrCpy $CodeHangarContextChoice 1
    Goto codehangar_choices_done
  codehangar_context_no:
    StrCpy $CodeHangarContextChoice 0

  codehangar_choices_done:
!macroend

!macro NSIS_HOOK_POSTINSTALL
  StrCpy $CodeHangarShellExecutable "$INSTDIR\${MAINBINARYNAME}.exe"
  WriteRegStr HKCU "Software\JCOM Labs\Code Hangar\Installations\${PRODUCTNAME}" "Executable" "$CodeHangarShellExecutable"
  ${If} $CodeHangarMarkdownChoice = 1
    !insertmacro CODEHANGAR_REGISTER_MARKDOWN
    WriteRegDWORD HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "MarkdownEnabled" 1
    ${If} $CodeHangarMarkdownWasEnabled <> 1
      WriteRegDWORD HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "DefaultGuidePending" 1
    ${EndIf}
  ${Else}
    !insertmacro CODEHANGAR_UNREGISTER_MARKDOWN
    WriteRegDWORD HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "MarkdownEnabled" 0
    WriteRegDWORD HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "DefaultGuidePending" 0
  ${EndIf}

  ${If} $CodeHangarContextChoice = 1
    !insertmacro CODEHANGAR_REGISTER_CONTEXT_MENU
    WriteRegDWORD HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "ContextMenuEnabled" 1
  ${Else}
    !insertmacro CODEHANGAR_UNREGISTER_CONTEXT_MENU
    WriteRegDWORD HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "ContextMenuEnabled" 0
  ${EndIf}

  ${If} $CodeHangarMarkdownChoice = 1
  ${OrIf} $CodeHangarContextChoice = 1
    WriteRegStr HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "OwnerExecutable" "$CodeHangarShellExecutable"
  ${Else}
    DeleteRegValue HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "OwnerExecutable"
  ${EndIf}
  !insertmacro CODEHANGAR_NOTIFY_SHELL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ; A user may have enabled resident startup later from Settings. Remove the
    ; Run value only when it still names the executable being uninstalled; never
    ; disturb another Code Hangar edition that has since taken ownership.
    ReadRegStr $3 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Code Hangar"
    StrCpy $4 "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" --background"
    ${If} $3 == $4
      DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Code Hangar"
    ${EndIf}

    DeleteRegKey HKCU "Software\JCOM Labs\Code Hangar\Installations\${PRODUCTNAME}"
    ReadRegStr $0 HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "OwnerExecutable"
    ${If} $0 == "$INSTDIR\${MAINBINARYNAME}.exe"
      StrCpy $CodeHangarShellExecutable ""
      ; Select any remaining valid Code Hangar installation without naming or
      ; embedding knowledge of another edition in this installer. The current
      ; product key was deleted above, and every candidate is revalidated before
      ; shared shell ownership is transferred.
      StrCpy $5 0
      codehangar_find_remaining_installation:
        EnumRegKey $6 HKCU "Software\JCOM Labs\Code Hangar\Installations" $5
        ${If} $6 == ""
          Goto codehangar_remaining_installation_done
        ${EndIf}
        ReadRegStr $1 HKCU "Software\JCOM Labs\Code Hangar\Installations\$6" "Executable"
        ${If} $1 != ""
        ${AndIf} $1 != "$INSTDIR\${MAINBINARYNAME}.exe"
        ${AndIf} ${FileExists} "$1"
          StrCpy $CodeHangarShellExecutable "$1"
          Goto codehangar_remaining_installation_done
        ${EndIf}
        IntOp $5 $5 + 1
        Goto codehangar_find_remaining_installation
      codehangar_remaining_installation_done:

      ${If} $CodeHangarShellExecutable != ""
        ReadRegDWORD $CodeHangarMarkdownChoice HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "MarkdownEnabled"
        ReadRegDWORD $CodeHangarContextChoice HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "ContextMenuEnabled"
        ${If} $CodeHangarMarkdownChoice = 1
          !insertmacro CODEHANGAR_REGISTER_MARKDOWN
        ${Else}
          !insertmacro CODEHANGAR_UNREGISTER_MARKDOWN
        ${EndIf}
        ${If} $CodeHangarContextChoice = 1
          !insertmacro CODEHANGAR_REGISTER_CONTEXT_MENU
        ${Else}
          !insertmacro CODEHANGAR_UNREGISTER_CONTEXT_MENU
        ${EndIf}
        WriteRegStr HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration" "OwnerExecutable" "$CodeHangarShellExecutable"
      ${Else}
        !insertmacro CODEHANGAR_UNREGISTER_MARKDOWN
        !insertmacro CODEHANGAR_UNREGISTER_CONTEXT_MENU
        DeleteRegKey HKCU "Software\JCOM Labs\Code Hangar\ShellIntegration"
      ${EndIf}
      !insertmacro CODEHANGAR_NOTIFY_SHELL
    ${EndIf}
    DeleteRegKey /ifempty HKCU "Software\JCOM Labs\Code Hangar\Installations"
    DeleteRegKey /ifempty HKCU "Software\JCOM Labs\Code Hangar"
    DeleteRegKey /ifempty HKCU "Software\JCOM Labs"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
