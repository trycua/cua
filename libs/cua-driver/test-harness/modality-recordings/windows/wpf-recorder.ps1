param([string]$Mode="ax-bg",[string]$Toolkit="wpf")   # Mode: ax-fg|ax-bg|vision-fg|vision-bg|vision-desktop ; Toolkit: wpf|winui3|webview2|electron
$TK=@{
 wpf     =@{exe="C:\Users\cuademo\cua\libs\cua-driver\rust\test-apps\harness-wpf\CuaTestHarness.Wpf.exe";        title="CuaTestHarness WPF";      label="WPF"}
 winui3  =@{exe="C:\Users\cuademo\cua\libs\cua-driver\rust\test-apps\harness-winui3\CuaTestHarness.WinUI3.exe";  title="CuaTestHarness WinUI3";   label="WinUI3"}
 webview2=@{exe="C:\Users\cuademo\cua\libs\cua-driver\rust\test-apps\harness-webview\CuaTestHarness.WebView.exe"; title="CuaTestHarness WebView"; label="WebView2"}
 electron=@{exe="C:\Users\cuademo\cua\libs\cua-driver\rust\test-apps\harness-electron\CuaTestHarness.Electron.exe";title="CuaTestHarness Electron";  label="Electron"}
}
$tk=$TK[$Toolkit]; if(-not $tk){ Write-Output "unknown toolkit $Toolkit"; exit 3 }
$ErrorActionPreference="Continue"
# screen is 1024x768 -> harness LEFT, dashboard panel RIGHT, both fully on-screen.
# Harness LEFT, dashboard panel RIGHT (1024x768). At this width the WPF form reflows
# taller than the screen, so the right-click button + scroll area sit off-screen; the
# cua-driver off-screen guard now reports those as a clean no-op (no taskbar misfire)
# instead of clicking the wrong target. Full-width avoids that but makes the harness
# the foreground window (breaks the no-foreground measurement) — so we keep this layout.
$HARX=0;$HARY=0;$HARW=556;$HARH=742
$PANX=560;$PANY=0;$PANW=462;$PANH=742
$META=@{
 "ax-fg"         =@{title="AX - FOREGROUND";  scope="window"; see="accessibility tree (element-level)"; fg=$true;  expect="App kept in FRONT on purpose. Each action runs via the accessibility tree; we measure the foreground."}
 "ax-bg"         =@{title="AX - BACKGROUND";  scope="window"; see="accessibility tree (element-level)"; fg=$false; expect="App should stay in the BACKGROUND. Each action runs via the accessibility tree; we measure which actions steal focus."}
 "vision-fg"     =@{title="VISION - FOREGROUND"; scope="window"; see="screenshot only (pixels)"; fg=$true;  expect="Pure pixel-driven, app kept in FRONT. We measure the foreground."}
 "vision-bg"     =@{title="VISION - BACKGROUND"; scope="window"; see="screenshot only (pixels)"; fg=$false; expect="Pure pixel-driven, app should stay in the BACKGROUND. We measure which pixel actions steal focus."}
 "vision-desktop"=@{title="VISION - FULL DESKTOP"; scope="desktop"; see="full-screen screenshot"; fg=$true; expect="Whole-screen, window-less screen-pixel actions (no window targeted)."}
}
$m=$META[$Mode]; if(-not $m){ Write-Output "unknown mode $Mode"; exit 2 }
$vision=($m.see -like 'screenshot*'); $desktop=($m.scope -eq 'desktop')
$dir="C:\Users\Public\cua-$Toolkit-$Mode"; $rec="$dir\rec"
Remove-Item $dir -Recurse -Force -EA SilentlyContinue; New-Item -ItemType Directory -Force $rec | Out-Null
# ---------- dashboard (compact for 462px) ----------
$html=@'
<!doctype html><html><head><meta charset="utf-8"><title>cua-driver-panel</title><style>
 html,body{margin:0;height:100%;font-family:Segoe UI,-apple-system,sans-serif;background:#0d1117;color:#e6edf3}.wrap{padding:12px}
 h1{font-size:11px;margin:0 0 1px;color:#8b949e}.run{font-size:17px;font-weight:800;color:#58a6ff;margin:0 0 5px}.exp{font-size:10.5px;color:#8b949e;margin:0 0 9px;line-height:1.35}
 .fg{padding:9px 11px;border-radius:9px;font-weight:800;font-size:12.5px;margin-bottom:6px;line-height:1.25}.fg.ok{background:#0f2e1a;color:#3fb950;border:1px solid #2ea043}.fg.bad{background:#3d1418;color:#ff6a5f;border:1px solid #da3633}.fg.fgmode{background:#16202e;color:#58a6ff;border:1px solid #2f4664}
 .sub{font-weight:500;font-size:9.5px;color:#c9d1d9;opacity:.85;margin-top:2px}.now{font-size:9.5px;font-family:Consolas,monospace;color:#6e7681;margin:5px 0 9px;word-break:break-all}
 .row{padding:6px 8px;border-radius:7px;margin:4px 0;font-size:11.5px;display:flex;align-items:center;gap:7px}.row.done{background:#11161d}.row.active{background:#161b22;border:1px solid #d29922}.row.pending{opacity:.5}
 .ic{width:14px;text-align:center}.lbl{flex:1}.res{font-size:9px;font-weight:700;padding:2px 5px;border-radius:5px;white-space:nowrap;margin-left:4px}.res.held{background:#0f2e1a;color:#3fb950}.res.stole{background:#3d1418;color:#ff6a5f}.res.front{background:#16202e;color:#58a6ff}
 .res.did{background:#10301c;color:#56d364;border:1px solid #238636}.res.nope{background:#3d1418;color:#ff7b72;border:1px solid #da3633}
 .legend{font-size:9px;color:#6e7681;margin:2px 0 8px;line-height:1.4}
 .tally{margin-top:10px;font-size:11px;color:#8b949e}.tally b{color:#c9d1d9}
</style></head><body><div class="wrap">
 <h1>cua-driver - single-modality run - TKLABEL</h1><div class="run" id="run">...</div><div class="exp" id="exp"></div>
 <div id="fg" class="fg fgmode">...</div><div class="now" id="now"></div>
 <div class="legend"><b style="color:#56d364">✓ worked</b> / <b style="color:#ff7b72">✗ no-op</b> = did the action change the app · held / STOLE = focus contract</div>
 <div id="rows"></div><div class="tally" id="tally"></div>
</div><script>
async function tick(){try{const r=await fetch('status.json',{cache:'no-store'});const s=await r.json();
 run.textContent=s.run||'';exp.textContent=s.expect||'';now.textContent='foreground now: '+(s.foreground||'?');
 if(s.fgmode){fg.className='fg fgmode';fg.innerHTML='FOREGROUND MODE - app intentionally in front<div class="sub">'+(s.appFront?'app is foreground':'...')+'</div>';}
 else if(s.appFront){fg.className='fg bad';fg.innerHTML='APP IS FOREGROUND<div class="sub">'+(s.foreground||'')+' - current action stole focus</div>';}
 else{fg.className='fg ok';fg.innerHTML='APP IN BACKGROUND<div class="sub">no-foreground contract holding - '+(s.foreground||'')+' stays frontmost</div>';}
 rows.innerHTML='';(s.steps||[]).forEach(st=>{const d=document.createElement('div');d.className='row '+st.state;
  const ic=st.state==='done'?'✓':st.state==='active'?'▶':'·';let res='';if(st.result==='held')res='<span class="res held">held</span>';else if(st.result==='stole')res='<span class="res stole">STOLE FOCUS</span>';else if(st.result==='front')res='<span class="res front">foreground</span>';
  let ver='';if(st.verified==='ok')ver='<span class="res did">✓ worked</span>';else if(st.verified==='fail')ver='<span class="res nope">✗ no-op</span>';
  d.innerHTML='<span class="ic">'+ic+'</span><span class="lbl">'+st.label+'</span>'+ver+res;rows.appendChild(d);});
 const worked=(s.steps||[]).filter(x=>x.verified==='ok').length;const ve=(s.steps||[]).filter(x=>x.verified==='ok'||x.verified==='fail').length;
 const eff='effects: <b>'+worked+'/'+ve+'</b> actions changed the app';
 if(!s.fgmode){tally.innerHTML=eff+'<br>focus contract: <b>'+(s.steals||0)+'/'+(s.actions||0)+'</b> stole focus';}else{tally.innerHTML=eff+'<br><b>'+(s.actions||0)+'</b> actions performed (foreground mode)';}
}catch(e){}}
setInterval(tick,200);tick();
</script></body></html>
'@
$html=$html -replace 'TKLABEL', $tk.label
Set-Content "$dir\dashboard.html" $html -Encoding UTF8
# ---------- loopback server ----------
$srv=Start-Job -ScriptBlock { param($d,$port)
 $l=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback,$port);$l.Start()
 while($true){try{$c=$l.AcceptTcpClient();$st=$c.GetStream();$rd=[IO.StreamReader]::new($st);$ln=$rd.ReadLine()
  if($ln -match 'GET\s+(\S+)'){$pp=($matches[1].TrimStart('/') -split '\?')[0];if($pp -eq ''){$pp='dashboard.html'}
   $fp=Join-Path $d $pp; if(Test-Path $fp){$b=[IO.File]::ReadAllBytes($fp);$ct=if($fp -like '*.html'){'text/html'}else{'application/json'}
    $h="HTTP/1.1 200 OK`r`nContent-Type: $ct`r`nContent-Length: $($b.Length)`r`nCache-Control: no-store`r`nConnection: close`r`n`r`n";$hb=[Text.Encoding]::ASCII.GetBytes($h);$st.Write($hb,0,$hb.Length);$st.Write($b,0,$b.Length)}}
  $st.Flush();$c.Close()}catch{}}
} -ArgumentList $dir,8146
Add-Type @"
using System;using System.Runtime.InteropServices;using System.Text;
public class W{[DllImport("user32.dll")]public static extern bool MoveWindow(IntPtr h,int x,int y,int w,int ht,bool r);
 [DllImport("user32.dll")]public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")]public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")]public static extern int GetWindowText(IntPtr h,StringBuilder s,int n);
 [DllImport("user32.dll")]public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int cx,int cy,uint flags);
 [DllImport("user32.dll")]public static extern bool ShowWindow(IntPtr h,int cmd);
 public static void Restore(IntPtr h){ ShowWindow(h,9); }
 public static void Topmost(IntPtr h,int x,int y,int cx,int cy){ SetWindowPos(h,(IntPtr)(-1),x,y,cx,cy,0x40); }
 public static string Title(IntPtr h){var sb=new StringBuilder(256);GetWindowText(h,sb,256);return sb.ToString();}}
"@
$drv="C:\Users\cuademo\cua\libs\cua-driver\rust\target\release\cua-driver.exe"; if(-not(Test-Path $drv)){$drv=$drv -replace 'release','debug'}
$wpf=$tk.exe
$chrome="C:\Program Files\Google\Chrome\Application\chrome.exe"
$recfwd=$rec -replace '\\','/'
function D($t,$j){ ($j | & $drv call $t 2>&1 | Out-String) }
function Els(){ ((D "get_window_state" ('{{"pid":{0},"window_id":{1},"capture_mode":"ax"}}' -f $script:wp,$script:wd))|ConvertFrom-Json).elements }
# ---------- per-action EFFECT verifier: read the WPF harness's own status labels via UIAutomation ----------
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
$script:AE=[System.Windows.Automation.AutomationElement]; $script:UTS=[System.Windows.Automation.TreeScope]; $script:UCT=[System.Windows.Automation.ControlType]
function ReadState(){
 try{
  $win=$null  # match by substring so cdp-suffixed web-harness titles ("... [cdp=9222]") resolve
  foreach($c in $script:AE::RootElement.FindAll($script:UTS::Children,[System.Windows.Automation.Condition]::TrueCondition)){ if("$($c.Current.Name)" -like "*$($tk.title)*"){ $win=$c; break } }
  if(-not $win){return @{}}
  $tc=New-Object System.Windows.Automation.PropertyCondition($script:AE::ControlTypeProperty,$script:UCT::Text)
  $all=(($win.FindAll($script:UTS::Descendants,$tc)|%{ $_.Current.Name }) -join " || ")
  $h=@{}
  if($all -match 'agreed=(\w+)'){$h.agreed=$matches[1]}
  if($all -match 'slider_value=(\d+)'){$h.slider=[int]$matches[1]}
  if($all -match 'last_action=(\w+)'){$h.last_action=$matches[1]}
  if($all -match 'mirror=([^|]*)'){$h.mirror=$matches[1].Trim()}
  if($all -match 'menu_action=(\w+)'){$h.menu=$matches[1]}
  if($all -match 'scroll_offset=(\d+)'){$h.scroll=[int]$matches[1]}
  return $h
 }catch{ return @{} }
}
function Verify($t,$b,$a){
 switch($t){
  'click'  { if("$($a.agreed)" -ne "$($b.agreed)"){'ok'}else{'fail'} }
  'double' { if("$($a.last_action)" -eq 'double_click'){'ok'}else{'fail'} }
  'right'  { if("$($a.menu)" -ne "$($b.menu)" -and "$($a.menu)" -ne 'none' -and "$($a.menu)" -ne ''){'ok'}else{'fail'} }
  'drag'   { if([int]$a.slider -gt [int]$b.slider){'ok'}else{'fail'} }
  'scroll' { if([int]$a.scroll -gt [int]$b.scroll){'ok'}else{'fail'} }
  'setval' { if("$($a.mirror)" -match 'set-by-cua'){'ok'}else{'fail'} }
  'type'   { if("$($a.mirror)" -match 'typed-by-cua'){'ok'}else{'fail'} }
  default  { 'na' }
 }
}
# ---------- action plan (filtered per mode) ----------
$plan=@(
 @{t='click';  sel='chk'; label='left-click a checkbox'}
 @{t='double'; sel='btn'; label='double-click a button'}
 @{t='right';  sel='ctx'; label='right-click (context menu)'}
 @{t='drag';   sel='sld'; label='drag the slider'}
 @{t='scroll'; sel='scr'; label='scroll the panel'}
 @{t='setval'; sel='txt'; label='set_value on the text box'}
 @{t='type';   sel='txt'; label='type into the text box'}
 @{t='key';    sel='txt'; label='press a key (Tab)'}
)
if($vision){ $plan=@($plan | ? { $_.t -ne 'setval' }) }                        # set_value is AX-only
if($desktop){ $plan=@($plan | ? { $_.t -in @('click','scroll','type','key') }) } # window-less supports click+scroll+global
$script:steals=0;$script:actions=0;$script:cur=""
$script:steps=@(); foreach($p in $plan){ $script:steps+=@{label=$p.label;state='pending';result='';verified=''} }
function Flush(){ $fgt=[W]::Title([W]::GetForegroundWindow());$af=($fgt -like '*CuaTestHarness*')
 (@{run=$m.title;expect=$m.expect;fgmode=$m.fg;foreground=$fgt;appFront=$af;steals=$script:steals;actions=$script:actions;steps=$script:steps}|ConvertTo-Json -Depth 6 -Compress)|Set-Content "$dir\status.json" -Encoding UTF8 }
function Pulse($sec){ $e=(Get-Date).AddSeconds($sec); while((Get-Date)-lt $e){ Flush; Start-Sleep -Milliseconds 150 } }
function Ctr($el){ ,@([int]($el.frame.x+$el.frame.w/2),[int]($el.frame.y+$el.frame.h/2)) }                 # screen center
function Win0($el){ ,@([int]($el.frame.x-$w.bounds.x+$el.frame.w/2),[int]($el.frame.y-$w.bounds.y+$el.frame.h/2)) } # window-local center
'{"run":"","expect":"","fgmode":true,"foreground":"","appFront":false,"steals":0,"actions":0,"steps":[]}' | Set-Content "$dir\status.json" -Encoding UTF8
# ---------- launch ----------
Start-Process $drv -ArgumentList "serve" -WindowStyle Hidden; Start-Sleep 4
D "set_agent_cursor_enabled" '{"enabled":true,"session":"d1"}'|Out-Null
D "set_agent_cursor_motion" '{"session":"d1","cursor_color":"#FF2D2D","cursor_label":"cua-driver","glide_duration_ms":600,"dwell_after_click_ms":700,"idle_hide_ms":120000}'|Out-Null
Start-Process $wpf; Start-Sleep (if($Toolkit -in @('electron','webview2')){16}else{5})  # web harnesses are slow to start
Start-Process $chrome -ArgumentList "--app=http://localhost:8146/","--user-data-dir=C:\Users\Public\cdp-$Mode","--no-first-run","--window-position=$PANX,$PANY","--window-size=$PANW,$PANH","--new-window"; Start-Sleep 4
# resolve harness window
$w=$null
for($i=0;$i -lt 20;$i++){ $w=(D "list_windows" "{}"|ConvertFrom-Json).windows | ? { $_.title -like "*$($tk.title)*" } | Select -First 1; if($w){break}; Start-Sleep -Milliseconds 500 }
if(-not $w){ "FATAL: harness window never appeared" | Set-Content "$dir\metric.log"; exit 1 }
$script:wp=$w.pid;$script:wd=$w.window_id;$hHar=[IntPtr][int64]$w.window_id
# layout: harness LEFT (un-maximize first), panel RIGHT + topmost. retry panel handle.
[W]::Restore($hHar)|Out-Null; Start-Sleep -Milliseconds 400; [W]::MoveWindow($hHar,$HARX,$HARY,$HARW,$HARH,$true)|Out-Null; Start-Sleep -Milliseconds 300; [W]::MoveWindow($hHar,$HARX,$HARY,$HARW,$HARH,$true)|Out-Null
$hPanel=[IntPtr]::Zero
for($i=0;$i -lt 16;$i++){ $hPanel=(Get-Process chrome -EA SilentlyContinue|?{$_.MainWindowTitle -like "*cua-driver-panel*"}|Select -First 1).MainWindowHandle; if($hPanel -and $hPanel -ne [IntPtr]::Zero){break}; Start-Sleep -Milliseconds 400 }
if($hPanel -and $hPanel -ne [IntPtr]::Zero){ [W]::MoveWindow($hPanel,$PANX,$PANY,$PANW,$PANH,$true)|Out-Null; [W]::Topmost($hPanel,$PANX,$PANY,$PANW,$PANH) }
"HANDLES hHar=$hHar hPanel=$hPanel screen=1024x768"|Set-Content "$dir\handles.log"
Start-Sleep 1
# re-read window bounds after the move (window-local coords need post-move origin)
$w2=(D "list_windows" "{}"|ConvertFrom-Json).windows | ? { $_.window_id -eq $script:wd } | Select -First 1; if($w2){ $w=$w2 }
# resolve control targets (post-move snapshot) with settle-retry
$E=$null;$resolve=$null
for($i=0;$i -lt 14;$i++){
  $E=Els
  $resolve=@{
   chk=($E|?{ "$($_.role)" -match 'Check' }|Select -First 1)
   btn=($E|?{ "$($_.label)" -match 'lick target' }|Select -First 1)
   ctx=($E|?{ "$($_.label)" -match 'context menu' }|Select -First 1)
   sld=($E|?{ "$($_.role)" -match 'Slider' }|Select -First 1)
   scr=($E|?{ "$($_.role)" -match 'Pane' -and "$($_.label)" -match 'scroll' }|Select -First 1)
   txt=($E|?{ "$($_.role)" -match 'Edit' }|Select -First 1)
  }
  if(-not $resolve.scr){ $resolve.scr=($E|?{ "$($_.role)" -match 'Pane' }|Select -Last 1) }
  if($resolve.chk){break}
  Start-Sleep -Milliseconds 700
}
"RESOLVE "+(($resolve.GetEnumerator()|%{"$($_.Key)=$([bool]$_.Value)"}) -join ' ')+" count=$(@($E).Count)"|Set-Content "$dir\resolve.log"
# seed the agent-cursor overlay BEFORE recording (off the controls)
D "move_cursor" ('{{"x":{0},"y":{1},"session":"d1"}}' -f ($HARW-30),($HARH-30))|Out-Null; Start-Sleep -Milliseconds 400
if($desktop){ D "set_config" '{"key":"capture_scope","value":"desktop"}'|Out-Null }
D "start_recording" ('{{"output_dir":"{0}","record_video":true}}' -f $recfwd)|Out-Null
Pulse 2
# one action, dispatched per mode (mirrors v1's proven Click: glide cursor, then act)
function DoAct($t,$el){
 if((-not $el) -and $t -notin @('type','key')){return}
 if($el){ $c=Ctr $el; D "move_cursor" ('{{"x":{0},"y":{1},"session":"d1"}}' -f $c[0],$c[1])|Out-Null; Start-Sleep -Milliseconds 550 }
 $wl= if($el){Win0 $el}else{$null}
 switch($t){
  'click'  { if($desktop){D "click" ('{{"x":{0},"y":{1},"session":"d1"}}' -f $c[0],$c[1])|Out-Null}
             elseif($vision){$disp=if($m.fg){'foreground'}else{'background'};D "click" ('{{"pid":{0},"window_id":{1},"x":{2},"y":{3},"dispatch":"{4}","session":"d1"}}' -f $wp,$wd,$wl[0],$wl[1],$disp)|Out-Null}
             else{D "click" ('{{"pid":{0},"window_id":{1},"element_index":{2},"session":"d1"}}' -f $wp,$wd,$el.element_index)|Out-Null} }
  'double' { if($vision){D "double_click" ('{{"pid":{0},"window_id":{1},"x":{2},"y":{3},"session":"d1"}}' -f $wp,$wd,$wl[0],$wl[1])|Out-Null}
             else{D "double_click" ('{{"pid":{0},"window_id":{1},"element_index":{2},"session":"d1"}}' -f $wp,$wd,$el.element_index)|Out-Null} }
  'right'  { if($vision){D "right_click" ('{{"pid":{0},"window_id":{1},"x":{2},"y":{3},"session":"d1"}}' -f $wp,$wd,$wl[0],$wl[1])|Out-Null}
             else{D "right_click" ('{{"pid":{0},"window_id":{1},"element_index":{2},"session":"d1"}}' -f $wp,$wd,$el.element_index)|Out-Null}
             Start-Sleep -Milliseconds 500; D "press_key" ('{{"pid":{0},"key":"escape","session":"d1"}}' -f $wp)|Out-Null }
  'drag'   { $fx=[int]($el.frame.x-$w.bounds.x+8);$fy=[int]($el.frame.y-$w.bounds.y+$el.frame.h/2);$tx=$fx+150; D "drag" ('{{"pid":{0},"from_x":{1},"from_y":{2},"to_x":{3},"to_y":{4},"session":"d1"}}' -f $wp,$fx,$fy,$tx,$fy)|Out-Null }
  'scroll' { if($desktop){D "scroll" ('{{"x":{0},"y":{1},"direction":"down","session":"d1"}}' -f $c[0],$c[1])|Out-Null}
             elseif($vision){D "scroll" ('{{"pid":{0},"window_id":{1},"x":{2},"y":{3},"direction":"down","session":"d1"}}' -f $wp,$wd,$wl[0],$wl[1])|Out-Null}
             else{D "scroll" ('{{"pid":{0},"window_id":{1},"element_index":{2},"direction":"down","session":"d1"}}' -f $wp,$wd,$el.element_index)|Out-Null} }
  'setval' { D "set_value" ('{{"pid":{0},"window_id":{1},"element_index":{2},"value":"set-by-cua","session":"d1"}}' -f $wp,$wd,$el.element_index)|Out-Null }
  'type'   { if($el){ D "click" ('{{"pid":{0},"window_id":{1},"element_index":{2},"session":"d1"}}' -f $wp,$wd,$el.element_index)|Out-Null; Start-Sleep -Milliseconds 350 }; D "type_text" ('{{"pid":{0},"text":"typed-by-cua","session":"d1"}}' -f $wp)|Out-Null }
  'key'    { D "press_key" ('{{"pid":{0},"key":"tab","session":"d1"}}' -f $wp)|Out-Null }
 }
}
# ---------- run ----------
for($i=0;$i -lt $plan.Count;$i++){
 $p=$plan[$i]; $el=$resolve[$p.sel]
 $script:steps[$i].state='active'; $script:cur=$p.label; Flush
 $before=ReadState
 DoAct $p.t $el
 # measure: sample foreground ~1.5s while the dashboard updates live
 $stole=$false; $e=(Get-Date).AddSeconds(1.5); while((Get-Date)-lt $e){ Flush; if([W]::Title([W]::GetForegroundWindow()) -like '*CuaTestHarness*'){$stole=$true}; Start-Sleep -Milliseconds 110 }
 $script:steps[$i].verified=(Verify $p.t $before (ReadState))
 $script:actions++
 if($m.fg){ $script:steps[$i].result='front' } elseif($stole){ $script:steals++;$script:steps[$i].result='stole' } else { $script:steps[$i].result='held' }
 $script:steps[$i].state='done'; Flush
 if($hPanel -and $hPanel -ne [IntPtr]::Zero){ [W]::Topmost($hPanel,$PANX,$PANY,$PANW,$PANH) }
}
$script:cur="done"; Pulse 2.5
if($desktop){ D "set_config" '{"key":"capture_scope","value":"window"}'|Out-Null }
D "stop_recording" "{}"|Out-Null; Start-Sleep 3
Stop-Job $srv -EA SilentlyContinue; Remove-Job $srv -Force -EA SilentlyContinue
Get-Process cua-driver,CuaTestHarness.Wpf,chrome -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
$mp4=Get-ChildItem $rec -Recurse -Filter *.mp4 -EA SilentlyContinue | Select -First 1
$verdict= if($m.fg){"foreground-mode, $($script:actions) actions"}else{"$($script:steals)/$($script:actions) actions stole focus"}
$worked=@($script:steps|?{$_.verified -eq 'ok'}).Count; $ver=@($script:steps|?{$_.verified -in @('ok','fail')}).Count
"MODE=$Mode MEASURE=$verdict EFFECTS=$worked/$ver`_landed MP4=$($mp4.FullName) SIZE=$($mp4.Length)" | Tee-Object "$dir\metric.log"
