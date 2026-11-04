-- /big <text>: says <text> in the current window as 5 lines of block
-- letters, drawn with reverse video (^V) like the old mIRC scripts.
-- Every cell is "_": plain = background, reversed = a block.  Spaces
-- would be trimmed at the end of a line, underscores never are.
--
-- /big -3d <text>: same letters in Unicode blocks with a "\u{2592}"
-- shadow cast down-left -- 6 lines.  Each dot is two cells wide while
-- the art fits in WIDTH_3D columns, one cell past that, so it doesn't
-- wrap in a normal-width window.  Needs a UTF-8 client on the other
-- end; the ^V style works everywhere.

local FONT = {
    A = { " ### ", "#   #", "#####", "#   #", "#   #" },
    B = { "#### ", "#   #", "#### ", "#   #", "#### " },
    C = { " ####", "#    ", "#    ", "#    ", " ####" },
    D = { "#### ", "#   #", "#   #", "#   #", "#### " },
    E = { "#####", "#    ", "#### ", "#    ", "#####" },
    F = { "#####", "#    ", "#### ", "#    ", "#    " },
    G = { " ####", "#    ", "#  ##", "#   #", " ####" },
    H = { "#   #", "#   #", "#####", "#   #", "#   #" },
    I = { "###", " # ", " # ", " # ", "###" },
    J = { "  ###", "   # ", "   # ", "#  # ", " ##  " },
    K = { "#   #", "#  # ", "###  ", "#  # ", "#   #" },
    L = { "#    ", "#    ", "#    ", "#    ", "#####" },
    M = { "#   #", "## ##", "# # #", "#   #", "#   #" },
    N = { "#   #", "##  #", "# # #", "#  ##", "#   #" },
    O = { " ### ", "#   #", "#   #", "#   #", " ### " },
    P = { "#### ", "#   #", "#### ", "#    ", "#    " },
    Q = { " ### ", "#   #", "# # #", "#  # ", " ## #" },
    R = { "#### ", "#   #", "#### ", "#  # ", "#   #" },
    S = { " ####", "#    ", " ### ", "    #", "#### " },
    T = { "#####", "  #  ", "  #  ", "  #  ", "  #  " },
    U = { "#   #", "#   #", "#   #", "#   #", " ### " },
    V = { "#   #", "#   #", "#   #", " # # ", "  #  " },
    W = { "#   #", "#   #", "# # #", "## ##", "#   #" },
    X = { "#   #", " # # ", "  #  ", " # # ", "#   #" },
    Y = { "#   #", " # # ", "  #  ", "  #  ", "  #  " },
    Z = { "#####", "   # ", "  #  ", " #   ", "#####" },
    ["0"] = { " ### ", "#  ##", "# # #", "##  #", " ### " },
    ["1"] = { " # ", "## ", " # ", " # ", "###" },
    ["2"] = { " ### ", "#   #", "  ## ", " #   ", "#####" },
    ["3"] = { "#### ", "    #", " ### ", "    #", "#### " },
    ["4"] = { "#   #", "#   #", "#####", "    #", "    #" },
    ["5"] = { "#####", "#    ", "#### ", "    #", "#### " },
    ["6"] = { " ### ", "#    ", "#### ", "#   #", " ### " },
    ["7"] = { "#####", "    #", "   # ", "  #  ", "  #  " },
    ["8"] = { " ### ", "#   #", " ### ", "#   #", " ### " },
    ["9"] = { " ### ", "#   #", " ####", "    #", " ### " },
    [" "] = { "  ", "  ", "  ", "  ", "  " },
    ["!"] = { "#", "#", "#", " ", "#" },
    ["?"] = { " ### ", "#   #", "  ## ", "     ", "  #  " },
    ["."] = { " ", " ", " ", " ", "#" },
    [","] = { "  ", "  ", "  ", " #", "# " },
    [":"] = { " ", "#", " ", "#", " " },
    ["-"] = { "    ", "    ", "####", "    ", "    " },
    ["+"] = { "     ", "  #  ", "#####", "  #  ", "     " },
    ["'"] = { "#", "#", " ", " ", " " },
    ["("] = { " #", "# ", "# ", "# ", " #" },
    [")"] = { "# ", " #", " #", " #", "# " },
    ["<"] = { "  #", " # ", "#  ", " # ", "  #" },
    [">"] = { "#  ", " # ", "  #", " # ", "#  " },
    ["/"] = { "    #", "   # ", "  #  ", " #   ", "#    " },
    ["#"] = { " # # ", "#####", " # # ", "#####", " # # " },
    ["@"] = { " ### ", "# ###", "# # #", "# ###", " ##  " },
    ["*"] = { "     ", "# # #", " ### ", "# # #", "     " },
    ["="] = { "    ", "####", "    ", "####", "    " },
    ["_"] = { "    ", "    ", "    ", "    ", "####" },
    ["^"] = { " # ", "# #", "   ", "   ", "   " },
}

local REV = "\22"
local CELL = "_"
local MAX_CHARS = 16
local MAX_CHARS_3D = 10
local WIDTH_3D = 60
local FULL = "\u{2588}"
local SHADE = "\u{2592}"

local function render(text)
    local rows = { {}, {}, {}, {}, {} }
    for ch in text:upper():gmatch(".") do
        local g = FONT[ch] or FONT["?"]
        for r = 1, 5 do
            table.insert(rows[r], g[r])
        end
    end
    local lines = {}
    for r = 1, 5 do
        local bits = table.concat(rows[r], " ")
        local out, on = {}, false
        for b in bits:gmatch(".") do
            local fill = (b == "#")
            if fill ~= on then
                table.insert(out, REV)
                on = fill
            end
            table.insert(out, CELL)
        end
        if on then
            table.insert(out, REV)
        end
        lines[r] = table.concat(out)
    end
    return lines
end

-- 5 rows of booleans, one per dot, letters one dot apart.
local function dots(text)
    local grid = { {}, {}, {}, {}, {} }
    local first = true
    for ch in text:upper():gmatch(".") do
        local g = FONT[ch] or FONT["?"]
        for r = 1, 5 do
            if not first then
                table.insert(grid[r], false)
            end
            for b in g[r]:gmatch(".") do
                table.insert(grid[r], b == "#")
            end
        end
        first = false
    end
    return grid
end

local function render_3d(text)
    local grid = dots(text)
    local width = #grid[1]
    local function at(r, c)
        return grid[r] ~= nil and grid[r][c] == true
    end
    -- one extra row below and one extra dot on the left for the shadow
    local scale = ((width + 1) * 2 <= WIDTH_3D) and 2 or 1
    local full, shade, blank = FULL:rep(scale), SHADE:rep(scale), (" "):rep(scale)
    local lines = {}
    for r = 1, 6 do
        local out = {}
        for c = 0, width do
            if at(r, c) then
                table.insert(out, full)
            elseif at(r - 1, c + 1) then
                table.insert(out, shade)
            else
                table.insert(out, blank)
            end
        end
        lines[r] = (table.concat(out):gsub("%s+$", ""))
    end
    return lines
end

irust.on_command("big", function(arg)
    arg = (arg or ""):gsub("^%s+", ""):gsub("%s+$", "")
    local three_d = false
    if arg == "-3d" or arg:match("^%-3d%s") then
        three_d = true
        arg = arg:sub(4):gsub("^%s+", "")
    end
    local max = three_d and MAX_CHARS_3D or MAX_CHARS
    if arg == "" then
        return "usage: /big [-3d] <text>  (block letters, max " .. MAX_CHARS
            .. " characters, " .. MAX_CHARS_3D .. " with -3d)"
    end
    if #arg > max then
        return "/big: at most " .. max .. " characters (the lines get too wide)"
    end
    for _, line in ipairs(three_d and render_3d(arg) or render(arg)) do
        irust.say(line)
    end
    return nil
end)
