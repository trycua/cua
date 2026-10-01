# cua command reference

Argument syntax for `cua do` and `cua trajectory` (from `cua <command> --help`).
Every command also takes `--json`.

## cua do

```text
One-shot computer actions against the selected target

Usage: cua do [OPTIONS] <COMMAND>

Commands:
  switch      Select the target: a sandbox name, `host`, `url <URL>`, or a legacy `<provider> <name>` pair
  status      Show the current target and zoom state
  ls          List targets (optionally of one provider)
  zoom        Crop screenshots to a window and map coordinates into it
  unzoom      Return to full-screen screenshots
  screenshot  Take a screenshot (saved to the temp directory unless --save)
  snapshot    Screenshot plus an AI summary of the screen and its interactive elements (needs ANTHROPIC_API_KEY)
  click       Click at image coordinates
  dclick      Double-click at image coordinates
  move        Move the cursor (image coordinates)
  type        Type text
  key         Press a key (enter, escape, tab, ...)
  hotkey      Keyboard shortcut (cmd+c, ctrl+shift+s)
  scroll      Scroll in a direction
  drag        Drag between two points (image coordinates)
  shell       Run a shell command, or open an interactive terminal (PTY) when no command is given on a terminal
  open        Open a file or URL
  launch      Launch an application
  window      Window management
  a11y        Accessibility tree and actions
  cursor      Print the cursor position (screen points)
  clipboard   Clipboard text

Options:
      --no-record                    Disable trajectory recording for this command
```

## cua do switch

```text
Select the target: a sandbox name, `host`, `url <URL>`, or a legacy `<provider> <name>` pair

Usage: cua do switch [OPTIONS] <TARGET> [NAME]

Arguments:
  <TARGET>  
  [NAME]    

Options:
      --token <TOKEN>                spacesd token (`url` and `host`) [env: CUA_ENV_TOKEN]
      --as <ALIAS>                   Sandbox name to register a `url` target under
```

## cua do status

```text
Show the current target and zoom state

Usage: cua do status [OPTIONS]

Options:
```

## cua do ls

```text
List targets (optionally of one provider)

Usage: cua do ls [OPTIONS] [PROVIDER]

Arguments:
  [PROVIDER]  

Options:
```

## cua do zoom

```text
Crop screenshots to a window and map coordinates into it

Usage: cua do zoom [OPTIONS] <WINDOW_NAME>

Arguments:
  <WINDOW_NAME>  

Options:
```

## cua do unzoom

```text
Return to full-screen screenshots

Usage: cua do unzoom [OPTIONS]

Options:
```

## cua do screenshot

```text
Take a screenshot (saved to the temp directory unless --save)

Usage: cua do screenshot [OPTIONS]

Options:
  -s, --save <SAVE>                  
```

## cua do snapshot

```text
Screenshot plus an AI summary of the screen and its interactive elements (needs ANTHROPIC_API_KEY)

Usage: cua do snapshot [OPTIONS] [INSTRUCTIONS]...

Arguments:
  [INSTRUCTIONS]...  

Options:
      --model <MODEL>                Model (default `claude-haiku-4-5`, or CUA_SNAPSHOT_MODEL)
```

## cua do click

```text
Click at image coordinates

Usage: cua do click [OPTIONS] <X> <Y> [BUTTON]

Arguments:
  <X>       
  <Y>       
  [BUTTON]  [default: left] [possible values: left, right, middle]

Options:
```

## cua do dclick

```text
Double-click at image coordinates

Usage: cua do dclick [OPTIONS] <X> <Y>

Arguments:
  <X>  
  <Y>  

Options:
```

## cua do move

```text
Move the cursor (image coordinates)

Usage: cua do move [OPTIONS] <X> <Y>

Arguments:
  <X>  
  <Y>  

Options:
```

## cua do type

```text
Type text

Usage: cua do type [OPTIONS] <TEXT>

Arguments:
  <TEXT>  

Options:
```

## cua do key

```text
Press a key (enter, escape, tab, ...)

Usage: cua do key [OPTIONS] <KEY>

Arguments:
  <KEY>  

Options:
```

## cua do hotkey

```text
Keyboard shortcut (cmd+c, ctrl+shift+s)

Usage: cua do hotkey [OPTIONS] <KEYS>

Arguments:
  <KEYS>  

Options:
```

## cua do scroll

```text
Scroll in a direction

Usage: cua do scroll [OPTIONS] <DIRECTION> [AMOUNT]

Arguments:
  <DIRECTION>  [possible values: up, down, left, right]
  [AMOUNT]     [default: 3]

Options:
```

## cua do drag

```text
Drag between two points (image coordinates)

Usage: cua do drag [OPTIONS] <X1> <Y1> <X2> <Y2>

Arguments:
  <X1>  
  <Y1>  
  <X2>  
  <Y2>  

Options:
```

## cua do shell

```text
Run a shell command, or open an interactive terminal (PTY) when no command is given on a terminal

Usage: cua do shell [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...  

Options:
      --cols <COLS>                  
      --rows <ROWS>                  
```

## cua do open

```text
Open a file or URL

Usage: cua do open [OPTIONS] <PATH>

Arguments:
  <PATH>  

Options:
```

## cua do launch

```text
Launch an application

Usage: cua do launch [OPTIONS] <APP> [ARGS]...

Arguments:
  <APP>      
  [ARGS]...  

Options:
```

## cua do window

```text
Window management

Usage: cua do window [OPTIONS] <COMMAND>

Commands:
  ls        List windows (optionally filtered by app or title)
  unfocus   Remove focus from the current window (presses Escape)
  focus     Focus a window
  minimize  Minimize a window
  maximize  Maximize a window
  restore   Restore a minimized or maximized window
  close     Close a window
  resize    Resize a window
  move      Move a window
  info      Show a window as JSON

Options:
```

## cua do a11y

```text
Accessibility tree and actions

Usage: cua do a11y [OPTIONS] <COMMAND>

Commands:
  tree  Print the accessibility tree (JSON)
  find  Find elements by name (and role)
  act   Act on an element: press, focus, set-value, increment, ...

Options:
```

## cua do cursor

```text
Print the cursor position (screen points)

Usage: cua do cursor [OPTIONS]

Options:
```

## cua do clipboard

```text
Clipboard text

Usage: cua do clipboard [OPTIONS] <COMMAND>

Commands:
  get   Print clipboard text
  set   Set clipboard text

Options:
```

## cua do-host-consent

```text
Grant consent for `cua do switch host`

Usage: cua do-host-consent [OPTIONS]

Options:
```

## cua trajectory ls

```text
List sessions

Usage: cua trajectory ls [OPTIONS] [MACHINE]

Arguments:
  [MACHINE]  

Options:
```

## cua trajectory view

```text
Zip, serve on loopback and open the hosted viewer

Usage: cua trajectory view [OPTIONS] [TARGET]

Arguments:
  [TARGET]  

Options:
  -p, --port <PORT>                  [default: 8089]
```

## cua trajectory stop

```text
Stop the file server started by `view`

Usage: cua trajectory stop [OPTIONS]

Options:
```

## cua trajectory clean

```text
Delete sessions

Usage: cua trajectory clean [OPTIONS]

Options:
      --older-than <DAYS>            
      --machine <MACHINE>            
  -y, --yes                          
```
