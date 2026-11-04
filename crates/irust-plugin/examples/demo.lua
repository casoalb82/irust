-- Demo script proving the plugin hook mechanism end to end -- not a
-- real feature, just something visibly different so a live test can
-- confirm the hook actually fires. A real script would return the same
-- kind of replacement string.
irust.on_message(function(nick, target, text)
    if text:match("^!echo ") then
        return "[echo] " .. text:sub(7)
    end
end)
