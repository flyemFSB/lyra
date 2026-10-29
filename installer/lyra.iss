; Lyra Windows Inno Setup 安装包制作脚本。
; 支持命令行传入参数：ISCC /DAppVersion=0.3.0 /DSourceDir=..\target\release /DOutputDir=..\target\dist lyra.iss

#ifndef AppVersion
  #define AppVersion "0.3.0"
#endif

#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif

#ifndef OutputDir
  #define OutputDir "..\target\dist"
#endif

#define AppName "Lyra音乐"
#define AppPublisher "Lyra Contributors"
#define AppURL "https://github.com/flyemFSB/lyra"
#define AppExeName "lyra.exe"

[Setup]
; 应用程序基础元数据
AppId={{D14CE3AA-92C8-4DE3-B78B-5FE6DFBC7D9A}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#AppURL}

; 安装路径与权限策略（默认免管理员权限安装，支持向导内切换为全局安装）
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

; 64位架构与现代化向导界面
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern

; 打包压缩配置与输出目标
OutputDir={#OutputDir}
OutputBaseFilename=Lyra-v{#AppVersion}-windows-x64-setup
Compression=lzma2/ultra64
SolidCompression=yes
SetupIconFile=logo.ico
UninstallDisplayIcon={app}\logo.ico

[Languages]
Name: "chinesesimp"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#SourceDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "logo.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"; IconFilename: "{app}\logo.ico"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; IconFilename: "{app}\logo.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
