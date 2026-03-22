local M = {}

local SHEEN_CMD = "sheen"

-- Per-buffer state: buf -> { job_id, port, scroll_timer, scroll_augroup }
local buffers = {}

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

  local cmd = { SHEEN_CMD, file }
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
    vim.notify("sheen: failed to start (is sheen in PATH?)", vim.log.levels.ERROR)
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

return M
