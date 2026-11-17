# CI-only integrated debug shell. Disposable own-repo selection only; no workspace/VM/credentials or injected Ready state.
param([Parameter(Mandatory)][string]$Executable, [Parameter(Mandatory)][string]$Output)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Hosted CI walk only; never run against an existing user profile' }
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms, System.Drawing, Accessibility
# Accessibility's legacy System.Runtime metadata needs the same reference-unification allowance as the .NET SDK.
Add-Type -ReferencedAssemblies 'System.Collections.dll', ([Accessibility.IAccessible].Assembly.Location) -CompilerOptions '/nowarn:1701' @'
using System;
using System.Runtime.InteropServices;
public static class NativeWalk {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string name);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string name);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, UIntPtr w, string l, uint flags, uint timeout, out UIntPtr result);
  private delegate bool EnumWindow(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] private static extern bool EnumChildWindows(IntPtr h, EnumWindow callback, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetClassName(IntPtr h, System.Text.StringBuilder name, int size);
  public static string[] ChildClasses(IntPtr root) {
    var classes = new System.Collections.Generic.List<string>();
    EnumChildWindows(root, (h, l) => {
      var cls = new System.Text.StringBuilder(256); GetClassName(h, cls, cls.Capacity);
      classes.Add(cls.ToString()); return true;
    }, IntPtr.Zero);
    return classes.ToArray();
  }
  [DllImport("oleacc.dll")] private static extern int AccessibleObjectFromWindow(IntPtr h, uint id, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out Accessibility.IAccessible accessible);
  [DllImport("oleacc.dll")] private static extern int AccessibleChildren(Accessibility.IAccessible accessible, int start, int count, [Out, MarshalAs(UnmanagedType.LPArray, SizeParamIndex=2)] object[] children, out int obtained);
  // Legacy Explorer toolbars may have no UIA children. Use MSAA, never toolbar process memory.
  // https://learn.microsoft.com/windows/win32/winauto/toolbar-control
  // https://learn.microsoft.com/windows/win32/api/oleacc/nf-oleacc-accessiblechildren
  // https://learn.microsoft.com/windows/win32/winauto/pop-up-menu
  public static int[] TrayPoint(IntPtr root, string[] names, string dump) {
    var windows = new System.Collections.Generic.List<IntPtr> { root };
    EnumChildWindows(root, (h, l) => { windows.Add(h); return true; }, IntPtr.Zero);
    var log = new System.Text.StringBuilder(); int[] point = null; int matches = 0;
    foreach (var h in windows) {
      var cls = new System.Text.StringBuilder(256); GetClassName(h, cls, cls.Capacity);
      bool visible = IsWindowVisible(h);
      log.AppendLine("HWND=" + h + "; Class=" + cls + "; Visible=" + visible);
      if (cls.ToString() != "ToolbarWindow32" && cls.ToString() != "Button" && cls.ToString() != "#32768") continue;
      Accessibility.IAccessible acc = null; object[] items = null;
      try {
        var iid = typeof(Accessibility.IAccessible).GUID;
        Marshal.ThrowExceptionForHR(AccessibleObjectFromWindow(h, 0xfffffffc, ref iid, out acc)); // OBJID_CLIENT
        int count = acc.accChildCount, obtained = 0; items = new object[count];
        log.AppendLine("MSAA children=" + count);
        if (count > 0) Marshal.ThrowExceptionForHR(AccessibleChildren(acc, 0, count, items, out obtained));
        for (int i = -1; i < obtained; i++) {
          object child = i < 0 ? (object)0 : items[i];
          var node = child as Accessibility.IAccessible ?? acc;
          object id = child is int ? child : 0;
          string label = node.get_accName(id); int state = Convert.ToInt32(node.get_accState(id));
          int x, y, width, height; node.accLocation(out x, out y, out width, out height, id);
          log.AppendLine("Child=" + i + "; Name=" + label + "; Role=" + node.get_accRole(id) + "; State=" + state + "; Rect=" + x + "," + y + "," + width + "," + height);
          // Reject unavailable, invisible and offscreen controls; click only an exact accessible name.
          if (Array.IndexOf(names, label) >= 0 && visible && (state & 0x18001) == 0 && width > 0 && height > 0) {
            matches++; point = new int[] { x + width / 2, y + height / 2 };
          }
        }
      } catch (COMException e) { log.AppendLine("MSAA error: " + e); }
      finally {
        if (items != null) foreach (var item in items) if (item != null && Marshal.IsComObject(item)) Marshal.ReleaseComObject(item);
        if (acc != null) Marshal.ReleaseComObject(acc);
      }
    }
    System.IO.File.WriteAllText(dump, log.ToString());
    if (matches > 1) throw new InvalidOperationException("More than one native notification control named " + String.Join("/", names));
    return point;
  }
}
'@
$Executable = (Resolve-Path $Executable).Path
$Output = [IO.Path]::GetFullPath($Output)
New-Item -ItemType Directory -Force $Output | Out-Null
# dirs::data_dir uses SHGetKnownFolderPath(RoamingAppData), not an APPDATA override.
$data = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'ai.orka.bridge'
$selected = Join-Path $data 'setup-v1/local-vm'
$vmData = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'ai.orka.bridge/local-vm-v1'
if (Test-Path $data) { throw "Expected fresh hosted-CI application profile: $data (will not erase it)" }
if (!(Test-Path $env:BRIDGE_DEV_IMAGE_DIR -PathType Container) -or @(Get-ChildItem -Force $env:BRIDGE_DEV_IMAGE_DIR).Count) { throw 'Expected empty image source directory' }
if (Get-Process -Name orka-bridge -ErrorAction SilentlyContinue) { throw 'Bridge is already running' }
$desktop = $null
$children = [System.Windows.Automation.TreeScope]::Descendants
function Wait-For([string]$Message, [scriptblock]$Predicate) {
    for ($i = 0; $i -lt 100; $i++) {
        $value = & $Predicate
        if ($value) { return $value }
        Start-Sleep -Milliseconds 100
    }
    throw $Message
}
function Find-Control($Root, [string]$Name, $Type) {
    $conditions = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, $Name)
    if ($Type) { $conditions = [System.Windows.Automation.AndCondition]::new($conditions, [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, $Type)) }
    $Root.FindFirst($children, $conditions)
}
$button = [System.Windows.Automation.ControlType]::Button
# A Places row action can carry its row name after the visible verb; match the verb as a leading word.
function Find-Action($Root, [string]$Label) {
    $buttons = $Root.FindAll($children, [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, $button))
    foreach ($candidate in $buttons) {
        $name = $candidate.Current.Name
        if ($null -eq $name) { continue }
        if ($name -ceq $Label -or $name.StartsWith("$Label ", [StringComparison]::Ordinal)) { return $candidate }
    }
}
function Get-UiNames($Root) {
    @($Root.FindAll($children, [System.Windows.Automation.Condition]::TrueCondition) | ForEach-Object { $_.Current.Name } | Where-Object { $_ })
}
$menuItem = [System.Windows.Automation.ControlType]::MenuItem
function Press($Element) {
    $pattern = $null
    if ($Element -isnot [Drawing.Point] -and $Element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) { $pattern.Invoke(); return }
    $point = if ($Element -is [Drawing.Point]) { $Element } else { $Element.GetClickablePoint() }
    [NativeWalk]::SetCursorPos([int]$point.X, [int]$point.Y) | Out-Null
    [NativeWalk]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    [NativeWalk]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
}
function Capture([string]$Name, [bool]$Menu = $false) {
    $r = if ($Menu) { [System.Windows.Forms.SystemInformation]::VirtualScreen } else { $script:window.Current.BoundingRectangle }
    $bitmap = [Drawing.Bitmap]::new([int]$r.Width, [int]$r.Height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen([int]$r.X, [int]$r.Y, 0, 0, $bitmap.Size)
        $bitmap.Save((Join-Path $script:themeOutput "$Name.png"), [Drawing.Imaging.ImageFormat]::Png)
        if (!$Menu) {
            # The maximized frame can extend below the display; sample the visible
            # window interior, not the themed taskbar or off-screen padding.
            $colour = $bitmap.GetPixel($bitmap.Width - 30, [int]($bitmap.Height / 2))
            $expected = if ($script:theme -eq 'dark') { @(31, 34, 38) } else { @(242, 241, 237) }
            $actual = @($colour.R, $colour.G, $colour.B)
            for ($channel = 0; $channel -lt 3; $channel++) {
                if ([Math]::Abs($actual[$channel] - $expected[$channel]) -ge 8) { throw 'Window screenshot did not show the requested Estuary page colour (or desktop capture unavailable)' }
            }
        }
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}
function Save-BrowserDiagnostics([string]$Name) {
    try {
        $browsers = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | ForEach-Object {
            # Keep only process/profile metadata, never the full command line or environment.
            $command = [string]$_.CommandLine
            $process = Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue
            [pscustomobject]@{
                pid = $_.ProcessId; parent = $_.ParentProcessId; created = $_.CreationDate
                cpuSeconds = $(if ($process) { $process.TotalProcessorTime.TotalSeconds })
                executable = $_.ExecutablePath
                profile = [regex]::Match($command, '(?:"--user-data-dir=(?<path>[^"]+)"|--user-data-dir="(?<path>[^"]+)"|--user-data-dir=(?<path>\S+))').Groups['path'].Value
                type = [regex]::Match($command, '--type=(\S+)').Groups[1].Value
            }
        })
        $versions = @(foreach ($root in @([Environment]::GetFolderPath('ProgramFilesX86'), [Environment]::GetFolderPath('LocalApplicationData'))) {
            Get-ChildItem (Join-Path $root 'Microsoft\EdgeWebView\Application\*\msedgewebview2.exe') -ErrorAction SilentlyContinue | ForEach-Object {
                [pscustomobject]@{ executable = $_.FullName; version = $_.VersionInfo.FileVersion }
            }
        })
        $classes = @()
        if ($script:hwnd -and $script:hwnd -ne [IntPtr]::Zero) { $classes = @([NativeWalk]::ChildClasses($script:hwnd)) }
        $startupMarkers = @()
        if ($script:startupLog -and (Test-Path $script:startupLog)) {
            $stream = [IO.File]::Open($script:startupLog, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
            $reader = [IO.StreamReader]::new($stream)
            try { $startupMarkers = @($reader.ReadToEnd() -split '\r?\n' | Where-Object { $_ -like 'bridge-startup:*' }) }
            finally { $reader.Dispose() }
        }
        [pscustomobject]@{
            captured = [DateTime]::UtcNow; hostPid = $(if ($script:app) { $script:app.Id } else { $null })
            hostCpuSeconds = $(if ($script:app -and !$script:app.HasExited) { $script:app.TotalProcessorTime.TotalSeconds })
            startupMarkers = $startupMarkers
            childClasses = @($classes); browsers = $browsers; installed = $versions
        } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $script:themeOutput "browser-$Name.json")
    } catch { Write-Warning "Browser diagnostics unavailable: $_" }
}
function Write-StartupEvent([string]$Event) {
    # Sample before writing so producer and observer times can expose delayed delivery.
    $epoch = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    try {
        $entry = [pscustomobject]@{
            event = $Event; epoch_ms = $epoch
            hostPid = $(if ($script:app) { $script:app.Id } else { $null })
            hwnd = $(if ($script:hwnd) { $script:hwnd.ToInt64() } else { 0 })
        } | ConvertTo-Json -Compress
        [IO.File]::AppendAllText((Join-Path $script:themeOutput 'startup-timeline.jsonl'), "$entry`n")
    } catch { Write-Warning "Startup timeline unavailable: $_" }
}
function Test-NativeReady {
    if (!(Test-Path $script:startupLog)) { return $false }
    $stream = [IO.File]::Open($script:startupLog, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    $reader = [IO.StreamReader]::new($stream)
    try { $startup = $reader.ReadToEnd() } finally { $reader.Dispose() }
    return $startup.Contains('bridge-startup: ready')
}
function Start-Bridge {
    if ((Test-Path $selected) -or (Test-Path $vmData)) { throw 'Saved local selection or VM data appeared; refusing to resume a VM' }
    $script:window = $null
    $script:desktop = $null
    $script:hwnd = [IntPtr]::Zero
    $script:app = $null
    $script:startupLog = $null
    Save-BrowserDiagnostics 'before-launch'
    $script:startupLog = Join-Path $script:themeOutput "app-$([Guid]::NewGuid().ToString('N')).err.log"
    Write-StartupEvent 'launch-begin'
    $script:app = Start-Process $Executable -NoNewWindow -PassThru -RedirectStandardOutput (Join-Path $script:themeOutput "app-$([Guid]::NewGuid().ToString('N')).out.log") -RedirectStandardError $script:startupLog
    Write-StartupEvent 'launch-returned'
    # Process.MainWindowHandle can select Tao's hidden event target. Identify
    # the real app frame by its configured title and this launch's process ID.
    $script:hwnd = Wait-For 'Main native window did not appear' {
        $h = [IntPtr]::Zero
        # $null becomes an empty C# string; NullString passes a native null class.
        while (($h = [NativeWalk]::FindWindowEx([IntPtr]::Zero, $h, [NullString]::Value, 'Bridge')) -ne [IntPtr]::Zero) {
            [uint32]$owner = 0
            [NativeWalk]::GetWindowThreadProcessId($h, [ref]$owner) | Out-Null
            if ($owner -eq $script:app.Id -and [NativeWalk]::IsWindowVisible($h)) { return $h }
        }
    }
    # This is first observation of the frame, not its producer-side creation time.
    Write-StartupEvent 'frame-observed'
}
function Find-ReadyClusters {
    if (!$script:window) {
        # Keep the original frame and control waits separate. Native setup proves
        # controller construction finished; the Clusters control, not page-load
        # completion, proves the frontend is ready for this walk.
        if (!(Test-NativeReady)) { return }
        Write-StartupEvent 'native-ready-observed'
        Write-StartupEvent 'uia-attach-begin'
        $script:desktop = [System.Windows.Automation.AutomationElement]::RootElement
        $script:window = [System.Windows.Automation.AutomationElement]::FromHandle($script:hwnd)
        Write-StartupEvent 'uia-attached'
        [NativeWalk]::SetForegroundWindow($script:hwnd) | Out-Null
    }
    Find-Control $script:window 'Clusters' $button
}
function Find-TrayPoint($Root, [string]$Label) {
    $all = @($Root.FindAll($children, [System.Windows.Automation.Condition]::TrueCondition))
    @("UIA descendants=$($all.Count)"; $all | ForEach-Object { "Name=$($_.Current.Name); Type=$($_.Current.ControlType.ProgrammaticName); Offscreen=$($_.Current.IsOffscreen)" }) | Set-Content (Join-Path $script:themeOutput "tray-$Label.txt")
    # Explorer may expose tooltip text with extra notification copy and a non-Button role.
    $matches = @($all | Where-Object { $_.Current.Name -like '*Bridge*' -and !$_.Current.IsOffscreen })
    if ($matches.Count -eq 1) { return $matches[0].GetClickablePoint() }
    if ($matches.Count -gt 1) { throw 'More than one Bridge icon in native notification area' }
    $p = [NativeWalk]::TrayPoint([IntPtr]$Root.Current.NativeWindowHandle, 'Bridge', (Join-Path $script:themeOutput "tray-$Label-msaa.txt"))
    if ($p) { return [Drawing.Point]::new($p[0], $p[1]) }
}
function Find-PopupControl([string]$Name) {
    $control = Find-Control $desktop $Name $menuItem
    if ($control -and $control.Current.IsEnabled -and !$control.Current.IsOffscreen) { return $control }
    $h = [IntPtr]::Zero
    while (($h = [NativeWalk]::FindWindowEx([IntPtr]::Zero, $h, '#32768', $null)) -ne [IntPtr]::Zero) {
        [uint32]$owner = 0
        [NativeWalk]::GetWindowThreadProcessId($h, [ref]$owner) | Out-Null
        if ($owner -ne $script:app.Id -or ![NativeWalk]::IsWindowVisible($h)) { continue }
        $p = [NativeWalk]::TrayPoint($h, $Name, (Join-Path $script:themeOutput "menu-$($Name.Replace(' ', '-'))-msaa.txt"))
        if ($p) { return [Drawing.Point]::new($p[0], $p[1]) }
    }
}
function Tray-Menu {
    $taskbar = [NativeWalk]::FindWindow('Shell_TrayWnd', $null)
    $notify = [NativeWalk]::FindWindowEx($taskbar, [IntPtr]::Zero, 'TrayNotifyWnd', $null)
    if ($notify -eq [IntPtr]::Zero) { throw 'Native notification area missing (interactive CI desktop required)' }
    $area = [System.Windows.Automation.AutomationElement]::FromHandle($notify)
    $point = Find-TrayPoint $area 'visible'
    if (!$point) {
        $chevron = Find-Control $area 'Show hidden icons' $button
        if (!$chevron) { $chevron = Find-Control $area 'Notification Chevron' $button }
        if (!$chevron) { $chevron = $area.FindFirst($children, [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::AutomationIdProperty, 'NotificationChevron')) }
        if (!$chevron) {
            $p = [NativeWalk]::TrayPoint($notify, @('Show hidden icons', 'Notification Chevron'), (Join-Path $script:themeOutput 'tray-chevron-msaa.txt'))
            if ($p) { $chevron = [Drawing.Point]::new($p[0], $p[1]) }
        }
        if (!$chevron) { throw 'Native notification chevron missing in UIA and MSAA' }
        Press $chevron
        $overflow = Wait-For 'Native notification overflow not visible' { $h = [NativeWalk]::FindWindow('NotifyIconOverflowWindow', $null); if ($h -ne [IntPtr]::Zero -and [NativeWalk]::IsWindowVisible($h)) { [System.Windows.Automation.AutomationElement]::FromHandle($h) } }
        $point = Wait-For 'Native Bridge notification icon missing' { Find-TrayPoint $overflow 'overflow' }
    }
    # Right-click the actual Explorer notification icon; do not invoke a product test command.
    [NativeWalk]::SetCursorPos([int]$point.X, [int]$point.Y) | Out-Null
    [NativeWalk]::mouse_event(8, 0, 0, 0, [UIntPtr]::Zero)
    [NativeWalk]::mouse_event(16, 0, 0, 0, [UIntPtr]::Zero)
    # Both providers require enabled, on-screen native items; MSAA yields real click points.
    $open = Wait-For 'Open Bridge menu item missing' { Find-PopupControl 'Open Bridge' }
    $stop = Wait-For 'Stop the local cluster and quit menu item missing' { Find-PopupControl 'Stop the local cluster and quit' }
    return @($open, $stop)
}
function Retained {
    $script:app.Refresh()
    if ($script:app.HasExited -or !(Get-Process -Id $script:originalPid -ErrorAction SilentlyContinue)) { throw 'Original Bridge process lost' }
    if (![NativeWalk]::IsWindow($script:hwnd) -or ![NativeWalk]::IsWindowVisible($script:hwnd) -or [NativeWalk]::IsIconic($script:hwnd)) { throw 'Original window not visible/unminimized' }
    Wait-For 'Reopen did not focus original process' {
        [uint32]$foregroundPid = 0
        [NativeWalk]::GetWindowThreadProcessId([NativeWalk]::GetForegroundWindow(), [ref]$foregroundPid) | Out-Null
        $foregroundPid -eq $script:originalPid
    } | Out-Null
    # File is a view toggle, not persisted in the document: a webview remount loses this editor.
    $editor = Find-Control $script:window 'Agent file' ([System.Windows.Automation.ControlType]::Edit)
    if (!$editor -or $editor.Current.IsOffscreen) { throw 'File editor view lost on reopen' }
    $value = $editor.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value
    if (!$value.Contains($script:marker)) { throw 'Edited text lost on reopen' }
    if (Test-Path $selected) { throw 'Walk accidentally selected a workspace' }
}
function Wait-Exit {
    if (!$script:app.WaitForExit(15000)) { throw 'Native quit did not exit within 15 seconds' }
    if ($script:app.ExitCode -ne 0) { throw "Native quit failed with exit code $($script:app.ExitCode)" }
}
$personalize = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
$existed = Test-Path $personalize
$prior = @{}
foreach ($name in @('AppsUseLightTheme', 'SystemUsesLightTheme')) { $prior[$name] = Get-ItemPropertyValue $personalize $name -ErrorAction SilentlyContinue }
function Broadcast-Theme {
    $result = [UIntPtr]::Zero
    [NativeWalk]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, 'ImmersiveColorSet', 2, 1000, [ref]$result) | Out-Null
}
try {
    if (!$existed) { New-Item $personalize -Force | Out-Null }
    foreach ($theme in @('light', 'dark')) {
        $script:theme = $theme
        $script:themeOutput = Join-Path $Output $theme
        New-Item -ItemType Directory -Force $script:themeOutput | Out-Null
        foreach ($name in $prior.Keys) { Set-ItemProperty $personalize $name ([int]($theme -eq 'light')) }
        Broadcast-Theme
        Start-Bridge
        $script:originalPid = $script:app.Id
        Write-StartupEvent 'clusters-wait-begin'
        try {
            $clusters = Wait-For 'Clusters button missing' { Find-ReadyClusters }
            Write-StartupEvent 'clusters-found'
        } catch {
            Write-StartupEvent 'clusters-wait-failed'
            throw
        }
        Press $clusters
        Save-BrowserDiagnostics 'loaded'
        # The dark walk reuses the light walk's profile, so the welcome may already be dismissed.
        Wait-For 'Welcome or Places did not appear' { (Find-Control $script:window 'Get started' $button) -or (Find-Action $script:window 'Set up') } | Out-Null
        $start = Find-Control $script:window 'Get started' $button
        if ($start) { Press $start }
        # Places owns the local entry: this computer's Set up action beside an empty cluster list.
        $setUp = Wait-For 'Set up action missing on Places' { $control = Find-Action $script:window 'Set up'; if ($control -and $control.Current.IsEnabled -and ((Get-UiNames $script:window) -match 'No other clusters added yet')) { $control } }
        Capture 'places-empty'
        Press $setUp
        $requirements = Wait-For 'Computer requirements panel missing' { Find-Control $script:window 'Computer requirements' $null }
        Wait-For 'Computer requirements did not settle' {
            $lines = (Get-UiNames $requirements) -join ' '
            $lines.Contains('These checks do not change this computer.') -or $lines.Contains('This computer has') -or $lines.Contains('Virtualization:') -or $lines.Contains("Bridge couldn't check")
        } | Out-Null
        # Required RAM and maximum data-disk growth stay visible; installed image size is disclosed.
        Press (Wait-For 'Workspace details disclosure missing' { Find-Control $script:window "What's inside" $button })
        Wait-For 'Local size facts did not settle' {
            $names = Get-UiNames $script:window
            $text = ($names -join ' ') -replace '\s+', ' '
            ($text -cmatch 'Needs virtualization and \d+\.\d GB of memory') -and ($text -cmatch 'image size is \d+\.\d GB after installation, not the amount downloaded') -and ($text -cmatch 'Data disk can grow to \d+\.\d GB') -and !($names -cmatch '^Reading|Unavailable in this build')
        } | Out-Null
        # Observe the real runner; never inject Ready or select a local workspace.
        Capture 'computer-requirements'
        if (Test-Path $selected) { throw 'Preflight selected a workspace' }
        Press (Wait-For 'Designer button missing' { Find-Control $script:window 'Designer' $button })
        # Observe the actual editable owned example before toggling File, not a
        # welcome-button proxy or a read-only preview that will be remounted.
        Wait-For 'Editable owned example missing' {
            $control = Find-Control $script:window 'Instructions' ([System.Windows.Automation.ControlType]::Edit)
            $value = $null
            if ($control -and !$control.Current.IsOffscreen -and $control.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$value) -and !$value.Current.IsReadOnly) { $true }
        } | Out-Null
        $file = Wait-For 'Enabled File button missing' { $control = Find-Control $script:window 'File' $button; if ($control -and $control.Current.IsEnabled -and !$control.Current.IsOffscreen) { $control } }
        # aria-pressed exposes Toggle rather than Invoke. Activate the exact
        # control semantically; a coordinate fallback can miss during layout.
        $toggle = $null
        if (!$file.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$toggle)) {
            $patterns = @($file.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName }) -join ', '
            throw "File button has no Toggle pattern; supported: $patterns"
        }
        Write-Host "File navigation: one Toggle action from $($toggle.Current.ToggleState)"
        $toggle.Toggle()
        $editor = Wait-For 'Editable loaded File view missing' {
            $control = Find-Control $script:window 'Agent file' ([System.Windows.Automation.ControlType]::Edit)
            $value = $null
            if ($control -and $control.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$value) -and !$value.Current.IsReadOnly) { $control }
        }
        $editor.SetFocus()
        # Invoke-based navigation can leave the WebView's DOM editor unfocused
        # even after UIA SetFocus. Establish real pointer/keyboard focus first.
        Press $editor
        Wait-For 'Native editor did not receive keyboard focus' { $editor.Current.HasKeyboardFocus } | Out-Null
        $script:marker = " lifecycle-$([Guid]::NewGuid().ToString('N'))"
        [System.Windows.Forms.SendKeys]::SendWait('^{END}')
        [System.Windows.Forms.SendKeys]::SendWait($script:marker)
        $owned = Join-Path ([Environment]::GetFolderPath('UserProfile')) 'Orka/agents/web-reader/web-reader.agent.md'
        Wait-For 'Native editor change not saved' {
            try { (Test-Path $owned) -and [IO.File]::ReadAllText($owned).Contains($script:marker) }
            catch [IO.IOException] {
                # Autosave can hold an exclusive writer; only sharing/lock violations are transient.
                if (($_.Exception.GetBaseException().HResult -band 0xffff) -notin 32, 33) { throw }
                $false
            }
        } | Out-Null
        $noticeMarker = Join-Path $data 'close-notice-seen-v1'
        $needsNotice = !(Test-Path $noticeMarker)
        # SC_CLOSE follows the native titlebar Close route, unlike process termination.
        [NativeWalk]::PostMessage($script:hwnd, 0x112, [IntPtr]0xf060, [IntPtr]::Zero) | Out-Null
        if ($needsNotice) {
            $notice = Wait-For 'First Close did not explain background running' { Find-Control $desktop 'Bridge is still running' $null }
            if ($notice.Current.ProcessId -ne $script:app.Id) { throw 'Close notice belongs to another process' }
            if (Test-Path $noticeMarker) { throw 'Close notice marked seen before acknowledgement' }
            Capture 'first-close-notice' $true
            $copy = (Get-UiNames $notice) -join ' '
            if (!$copy.Contains('system tray') -or !$copy.Contains('Stop the local cluster and quit')) { throw 'Close notice lost reopen/Quit guidance' }
            # TaskDialog exposes its OK action as a Pane on this runner. Keep the exact
            # label scoped to the already verified same-process notice; Press uses its native clickable point.
            Press (Wait-For 'Close notice OK missing' { Find-Control $notice 'OK' $null })
        }
        Wait-For 'Native Close did not hide' { ![NativeWalk]::IsWindowVisible($script:hwnd) } | Out-Null
        if (!(Test-Path $noticeMarker) -or (Find-Control $desktop 'Bridge is still running' $null)) { throw 'Acknowledged notice was not remembered/dismissed' }
        if ($script:app.HasExited) { throw 'Close quit instead of hiding' }
        $commands = Tray-Menu
        Capture 'menu-hidden' $true
        Press $commands[0]
        Wait-For 'Open Bridge did not reopen' { [NativeWalk]::IsWindowVisible($script:hwnd) } | Out-Null
        Retained; Capture 'reopened-menu'
        [NativeWalk]::PostMessage($script:hwnd, 0x112, [IntPtr]0xf060, [IntPtr]::Zero) | Out-Null
        Wait-For 'Close before second launch did not hide' { ![NativeWalk]::IsWindowVisible($script:hwnd) } | Out-Null
        if (Find-Control $desktop 'Bridge is still running' $null) { throw 'Second Close repeated the information dialog' }
        $second = Start-Process $Executable -NoNewWindow -PassThru
        if (!$second.WaitForExit(15000) -or $second.ExitCode -ne 0) { throw 'Hidden-window second launch did not hand off and exit cleanly' }
        Wait-For 'Second launch did not show hidden original window' { [NativeWalk]::IsWindowVisible($script:hwnd) } | Out-Null
        Retained; Capture 'reopened-second-launch-hidden'
        [NativeWalk]::ShowWindow($script:hwnd, 6) | Out-Null
        Wait-For 'Window did not minimize' { [NativeWalk]::IsIconic($script:hwnd) } | Out-Null
        $second = Start-Process $Executable -NoNewWindow -PassThru
        if (!$second.WaitForExit(15000) -or $second.ExitCode -ne 0) { throw 'Second executable launch did not hand off and exit cleanly' }
        Wait-For 'Second launch did not restore original window' { ![NativeWalk]::IsIconic($script:hwnd) } | Out-Null
        Retained; Capture 'reopened-second-launch-minimized'
        if (@(Get-Process -Name orka-bridge).Count -ne 1) { throw 'A second Bridge process survived' }
        # Append creation after retention checks so their sample document and File view stay unchanged.
        $repo = Join-Path $env:RUNNER_TEMP "bridge-own-repo-$theme-$([Guid]::NewGuid().ToString('N'))"
        New-Item -ItemType Directory -Path $repo | Out-Null
        & git init --quiet $repo # Fixture setup only; the product must never invoke Git for this storage.
        if ($LASTEXITCODE -ne 0) { throw 'Cannot initialize disposable own-repo fixture' }
        Press (Wait-For 'New agent button missing' { Find-Control $script:window 'New agent' $button })
        $name = Wait-For 'New agent Name field missing' { Find-Control $script:window 'Name' ([System.Windows.Automation.ControlType]::Edit) }
        Press $name
        [System.Windows.Forms.SendKeys]::SendWait('web-reader')
        Press (Wait-For 'Own-repo choice did not settle' {
            $control = Find-Control $script:window 'Your own repo' ([System.Windows.Automation.ControlType]::RadioButton)
            if ($control -and $control.Current.IsEnabled -and !$control.Current.IsOffscreen) { $control }
        })
        $create = Wait-For 'Own-repo Create did not disable' {
            $control = Find-Control $script:window 'Create' $button
            if ($control -and !$control.Current.IsEnabled) { $control }
        }
        if ($create.Current.IsEnabled) { throw 'Own-repo Create must wait for a native folder selection' }
        Press (Wait-For 'Choose repo button missing' { Find-Control $script:window 'Choose repo' $button })
        $dialog = Wait-For 'Native repo folder chooser missing' {
            $control = Find-Control $desktop 'Choose your repo' ([System.Windows.Automation.ControlType]::Window)
            if ($control -and $control.Current.ProcessId -eq $script:app.Id -and !$control.Current.IsOffscreen) { $control }
        }
        [NativeWalk]::SetForegroundWindow([IntPtr]$dialog.Current.NativeWindowHandle) | Out-Null
        # Drive the actual IFileOpenDialog address bar, not a renderer picker reply.
        [System.Windows.Forms.SendKeys]::SendWait('^l')
        # The hosted shell exposes the focused address editor as a Pane without
        # ValuePattern. Type into its Ctrl+L selection, as shown by native capture.
        if ([NativeWalk]::GetForegroundWindow() -ne [IntPtr]$dialog.Current.NativeWindowHandle) { throw 'Native repo chooser lost focus' }
        [System.Windows.Forms.SendKeys]::SendWait($repo)
        [System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
        Wait-For 'Native folder chooser did not navigate to the disposable repo' {
            (Get-UiNames $dialog) -ccontains (Split-Path $repo -Leaf)
        } | Out-Null
        Capture 'new-agent-repo-chooser' $true
        # IDOK is the native folder selection action, independent of its system wording.
        $choose = Wait-For 'Native folder chooser selection action missing' {
            $control = $dialog.FindFirst($children, [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::AutomationIdProperty, '1'))
            if ($control -and $control.Current.IsEnabled -and !$control.Current.IsOffscreen) { $control }
        }
        Press $choose
        $folder = Join-Path $repo 'web-reader'
        # Rust's canonicalized drive path includes the extended-length prefix.
        $displayFolder = "\\?\$folder\"
        $copy = 'Files save automatically. You manage commits in this repo.'
        Wait-For 'Selected repo path or own-repo saving explanation missing' {
            $path = Find-Control $script:window $displayFolder $null
            $explanation = Find-Control $script:window $copy $null
            $change = Find-Control $script:window 'Change repo' $button
            $path -and !$path.Current.IsOffscreen -and $explanation -and !$explanation.Current.IsOffscreen -and $change -and $change.Current.IsEnabled
        } | Out-Null
        Capture 'new-agent-repo-selected'
        Press (Find-Control $script:window 'Create' $button)
        $agent = Join-Path $folder 'web-reader.agent.md'
        $tests = Join-Path $folder 'web-reader.agent.test.md'
        Wait-For 'Own-repo web-reader pair or created Agent view missing' {
            $editor = Find-Control $script:window 'Instructions' ([System.Windows.Automation.ControlType]::Edit)
            $explanation = Find-Control $script:window $copy $null
            (Test-Path $agent) -and (Test-Path $tests) -and $editor -and !$editor.Current.IsOffscreen -and $explanation -and !(Find-Control $script:window 'Create' $button)
        } | Out-Null
        $draft = [IO.File]::ReadAllText($agent).Replace("`r`n", "`n")
        $checks = [IO.File]::ReadAllText($tests).Replace("`r`n", "`n")
        if (!$draft.Contains('name: "web-reader"') -or $draft.Contains('tools:') -or $draft -notmatch '(?s)\A---\n.*?\n---\n\s*\z' -or $draft.Contains('web_fetch')) { throw 'Own-repo draft must have empty instructions and no tools' }
        $expectedChecks = @'
---
schemaVersion: 1
---

# web-reader tests

```yaml agent-test
id: first-test
input: ""
assertions: []
```
'@
        if ($checks.Trim() -cne $expectedChecks.Replace("`r`n", "`n").Trim()) { throw 'Own-repo empty test skeleton has incorrect contents' }
        $instructions = Find-Control $script:window 'Instructions' ([System.Windows.Automation.ControlType]::Edit)
        $value = $instructions.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value
        if ($value -cne '') { throw 'Created agent must open blank Instructions, not the previous File view' }
        if ((@(Get-ChildItem -Force $folder | Sort-Object Name | ForEach-Object Name) -join ',') -cne 'web-reader.agent.md,web-reader.agent.test.md') { throw 'Own-repo agent folder must contain only the pair, no nested .git or marker' }
        if ((@(Get-ChildItem -Force $repo | Sort-Object Name | ForEach-Object Name) -join ',') -cne '.git,web-reader') { throw 'Creation added unexpected repo-root files or markers' }
        Capture 'new-agent-repo-created'
        # The next theme reuses this profile. Native list() puts the original sample first.
        $selector = Find-Control $script:window 'Agent' ([System.Windows.Automation.ControlType]::ComboBox)
        Press $selector
        [System.Windows.Forms.SendKeys]::SendWait('{HOME}{ENTER}')
        Wait-For 'Original sample selection was not restored' { Find-Control $script:window 'Saved on this computer with local Git history. Nothing is uploaded.' $null } | Out-Null
        $commands = Tray-Menu
        Capture 'menu-stop' $true
        Press $commands[1]; Wait-Exit
        # File > Quit is exercised by the native menu/failed-stop subprocess test,
        # not by a duplicate GUI lookup through this runner's incomplete UIA tree.
        if ((Test-Path $selected) -or (Test-Path $vmData)) { throw 'Saved local selection or VM data appeared' }
        "PASS $theme`: native own-repo chooser, selected path/copy, blank web-reader-named draft pair only (no nested Git/marker); native first-Close notice/acknowledgement when unseen, profile marker and second Close without dialog, Close/hide, tray menu/Open, same PID $script:originalPid/window $script:hwnd, File view/edited text retained, hidden/minimized second launches, menu stop. Standard Quit is covered by native subprocess tests. Integrated debug shell only; empty image source, no VM/Ready/provider/cluster/session logout proof. OS popup theme is captured as drawn, not forced by product hooks." | Tee-Object (Join-Path $script:themeOutput 'evidence.txt')
    }
} catch {
    $failure = $_
    if ($script:themeOutput) {
        Save-BrowserDiagnostics 'failure'
        try {
            if (!$script:window) { throw 'UIA not attached: native startup did not complete' }
            $all = @($script:window.FindAll($children, [System.Windows.Automation.Condition]::TrueCondition))
            @("Root Name=$($script:window.Current.Name); PID=$($script:window.Current.ProcessId); HWND=$($script:window.Current.NativeWindowHandle); Class=$($script:window.Current.ClassName); Descendants=$($all.Count)"; $all | Select-Object -First 100 | ForEach-Object { "Name=$($_.Current.Name); Type=$($_.Current.ControlType.ProgrammaticName); Enabled=$($_.Current.IsEnabled); Offscreen=$($_.Current.IsOffscreen)" }) | Set-Content (Join-Path $script:themeOutput 'failure-ui.txt')
        } catch { Write-Warning $_ }
        try { Capture 'failure' $true } catch { Write-Warning $_ }
        if (!$script:window -and $script:app -and !$script:app.HasExited) {
            # The walk has already failed. Observe that same process, without
            # retrying startup or accepting late readiness, before cleanup.
            Start-Sleep -Seconds 5
            Save-BrowserDiagnostics 'failure-after-5s'
            Start-Sleep -Seconds 15
            Save-BrowserDiagnostics 'failure-after-20s'
        }
    }
    throw $failure
} finally {
    # Force cleanup only on test failure; it is not evidence that the stop route worked.
    if ($script:app -and !$script:app.HasExited) { Stop-Process -Id $script:app.Id -Force -ErrorAction SilentlyContinue }
    foreach ($name in $prior.Keys) {
        if ($null -eq $prior[$name]) { Remove-ItemProperty $personalize $name -ErrorAction SilentlyContinue }
        else { Set-ItemProperty $personalize $name $prior[$name] }
    }
    if (!$existed) { Remove-Item $personalize -ErrorAction SilentlyContinue }
    Broadcast-Theme
}
