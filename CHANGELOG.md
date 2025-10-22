* **0.1.0-beta.12**
    * added `User Views`
    * fixed longstanding bug where application wouldn't exit cleanly and required hard boot
    * added a UDP OSC listener
        * port defaults to: 2345
        * lets you send `/rnboctl/` messages directly from your host instead of having to use RNBO.
* **0.1.0-beta.11**
    * added new osc routes
        * `/rnboctl/device/params <device index> [<optional page index>]`
        * `/rnboctl/device/data <device index>`
* **0.1.0-beta.10**
    * using param `steps` and `enum` values to inform normalized parameter updates and better reach all steps of enum and stepped values
    * updated param pager display to use scrollbar
    * hiding params from device params and default param views that have @meta `{ "hidden": true }`
    * simplified param rendering
        * should fix cases where some param lights were lit mistakenly when there was no param present
    * disabled navigating to an empty Param View
    * displaying message in Param View menu when there are none to list
* **0.1.0-beta.9**
    * added Graph Preset Features
        * Save (currently the name is automatically created from the current time)
        * Delete
        * Overwrite
        * Set Initial (this simply renames a preset to the name "initial"
    * added a Popup that shows for various Preset actions
    * Removed menu dive for Param View when there is only 1
* **0.1.0-beta.8**
    * updated the UI to use [mousefood](https://github.com/j-g00da/mousefood) and [ratatui](https://github.com/ratatui/ratatui)
        * using [spleen font](https://github.com/fcambus/spleen)
    * added `Patchers` menu item that lets you directly load a patcher, just like you would when you "Run on Selected Target" via the Max sidebar

