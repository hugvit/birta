local M = {}

local config = {
  cmd = "sheen",
  theme = nil,
  syntax_theme = nil,
  css = nil,
  port = nil,
  no_open = false,
  theme_swap = false,
  no_toggle = false,
  no_header = false,
  font_body = nil,
  font_mono = nil,
}

-- Per-buffer state: buf -> { job_id, port, scroll_timer, scroll_augroup }
local buffers = {}

local function build_cmd(file)
  local cmd = { config.cmd }
  if config.theme then table.insert(cmd, "--theme"); table.insert(cmd, config.theme) end
  if config.syntax_theme then table.insert(cmd, "--syntax-theme"); table.insert(cmd, config.syntax_theme) end
  if config.css then table.insert(cmd, "--css"); table.insert(cmd, config.css) end
  if config.port then table.insert(cmd, "--port"); table.insert(cmd, tostring(config.port)) end
  if config.no_open then table.insert(cmd, "--no-open") end
  if config.theme_swap then table.insert(cmd, "--theme-swap") end
  if config.no_toggle then table.insert(cmd, "--no-toggle") end
  if config.no_header then table.insert(cmd, "--no-header") end
  if config.font_body then table.insert(cmd, "--font-body"); table.insert(cmd, config.font_body) end
  if config.font_mono then table.insert(cmd, "--font-mono"); table.insert(cmd, config.font_mono) end
  table.insert(cmd, file)
  return cmd
end

local function setup_scroll(buf, port)
  local timer = vim.uv.new_timer()
  local last_line = 0
  local augroup = vim.api.nvim_create_augroup("SheenScroll" .. buf, { clear = true })

  vim.api.nvim_create_autocmd({ "CursorMoved", "CursorHold" }, {
    group = augroup,
    buffer = buf,
    callback = function()
      local line = vim.api.nvim_win_get_cursor(0)[1]
      if line == last_line then return end
      last_line = line
      timer:stop()
      timer:start(50, 0, vim.schedule_wrap(function()
        vim.system({
          "curl", "-s", "-o", "/dev/null", "-X", "POST",
          string.format("http://127.0.0.1:%d/scroll/%d", port, line),
        })
      end))
    end,
  })

  local state = buffers[buf]
  if state then
    state.scroll_timer = timer
    state.scroll_augroup = augroup
  end
end

local function teardown_scroll(buf)
  local state = buffers[buf]
  if not state then return end
  if state.scroll_timer then
    state.scroll_timer:stop()
    state.scroll_timer:close()
    state.scroll_timer = nil
  end
  if state.scroll_augroup then
    vim.api.nvim_del_augroup_by_id(state.scroll_augroup)
    state.scroll_augroup = nil
  end
end

--- Stop all running previews on Neovim exit.
vim.api.nvim_create_autocmd("VimLeavePre", {
  callback = function()
    for buf, state in pairs(buffers) do
      teardown_scroll(buf)
      if state.job_id then
        vim.fn.jobstop(state.job_id)
      end
    end
    buffers = {}
  end,
})

--- Start preview for the current buffer.
function M.preview()
  local buf = vim.api.nvim_get_current_buf()

  if buffers[buf] then
    vim.notify("sheen: already running for this buffer", vim.log.levels.WARN)
    return
  end

  local file = vim.api.nvim_buf_get_name(buf)
  if file == "" then
    vim.notify("sheen: buffer has no file", vim.log.levels.ERROR)
    return
  end

  local state = { job_id = nil, port = nil, scroll_timer = nil, scroll_augroup = nil }
  buffers[buf] = state

  local cmd = build_cmd(file)
  local job_id = vim.fn.jobstart(cmd, {
    on_stderr = function(_, data)
      for _, line in ipairs(data) do
        local p = line:match("http://127%.0%.0%.1:(%d+)")
        if p and not state.port then
          state.port = tonumber(p)
          vim.schedule(function()
            setup_scroll(buf, state.port)
          end)
        end
      end
    end,
    on_exit = function(_, code)
      vim.schedule(function()
        teardown_scroll(buf)
        buffers[buf] = nil
        if code ~= 0 and code ~= 143 then
          vim.notify("sheen: exited with code " .. code, vim.log.levels.WARN)
        end
      end)
    end,
  })

  if job_id <= 0 then
    buffers[buf] = nil
    vim.notify("sheen: failed to start (is " .. config.cmd .. " in PATH?)", vim.log.levels.ERROR)
    return
  end

  state.job_id = job_id

  vim.api.nvim_create_autocmd("BufDelete", {
    buffer = buf,
    once = true,
    callback = function()
      M.stop(buf)
    end,
  })
end

--- Stop preview for a buffer (defaults to current).
---@param buf? number
function M.stop(buf)
  buf = buf or vim.api.nvim_get_current_buf()
  local state = buffers[buf]
  if not state then
    vim.notify("sheen: not running for this buffer", vim.log.levels.WARN)
    return
  end
  teardown_scroll(buf)
  if state.job_id then
    vim.fn.jobstop(state.job_id)
  end
  buffers[buf] = nil
end

--- Toggle preview for the current buffer.
function M.toggle()
  local buf = vim.api.nvim_get_current_buf()
  if buffers[buf] then
    M.stop(buf)
  else
    M.preview()
  end
end

--- Check if preview is running for a buffer (defaults to current).
---@param buf? number
---@return boolean
function M.is_running(buf)
  buf = buf or vim.api.nvim_get_current_buf()
  return buffers[buf] ~= nil
end

--- Configure sheen options. Call before :SheenPreview.
---@param opts? table
function M.setup(opts)
  config = vim.tbl_deep_extend("force", config, opts or {})
end

return M
