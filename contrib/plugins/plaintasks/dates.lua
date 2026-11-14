-- Dates: the stamps of @done, @cancelled, @started, @toggle and @created, the short dates of
-- @due, and the time spent on a task (@lasted, @wasted).
--
-- A date is a table { year, month, day, hour, min } (like os.date("*t") gives); times are
-- seconds since the epoch (like os.time gives), in local time.

local M = {}

local MONTHS = {
  jan = 1, feb = 2, mar = 3, apr = 4, may = 5, jun = 6,
  jul = 7, aug = 8, sep = 9, oct = 10, nov = 11, dec = 12,
}

-- The time of `date`, or nil when it is not a real date (February 30th, hour 25).
function M.time(date)
  local fields = {
    year = date.year, month = date.month, day = date.day,
    hour = date.hour or 0, min = date.min or 0, sec = 0,
  }
  -- os.time normalizes (February 30th becomes March 1st, in `fields` too): such a date is
  -- not one
  local ok, time = pcall(os.time, fields)
  if not ok or not time then
    return nil
  end
  local back = os.date("*t", time)
  if back.year ~= date.year or back.month ~= date.month or back.day ~= date.day
    or back.hour ~= (date.hour or 0) or back.min ~= (date.min or 0) then
    return nil
  end
  return time
end

-- `time` written with `format` (strftime directives: "(%y-%m-%d %H:%M)").
function M.format(time, format)
  return os.date(format, time)
end

-- `text` read with `format`, the whole of it (like Python's strptime): a date, or nil.
function M.strptime(text, format)
  local date = {}
  local position = 1
  local i = 1
  while i <= #format do
    local char = format:sub(i, i)
    if char == "%" and i < #format then
      local directive = format:sub(i + 1, i + 1)
      i = i + 2
      local digits = ({ Y = 4, y = 2, m = 2, d = 2, H = 2, M = 2, S = 2, I = 2 })[directive]
      if digits then
        local value = text:match("^%d%d?%d?%d?", position)
        if not value then
          return nil
        end
        value = value:sub(1, digits)
        position = position + #value
        local number = tonumber(value)
        if directive == "Y" then
          date.year = number
        elseif directive == "y" then
          date.year = number + (number < 69 and 2000 or 1900)
        elseif directive == "m" then
          date.month = number
        elseif directive == "d" then
          date.day = number
        elseif directive == "H" or directive == "I" then
          date.hour = number
        elseif directive == "M" then
          date.min = number
        end
      elseif directive == "b" or directive == "B" then
        local name = text:match("^%a+", position)
        local month = name and MONTHS[name:sub(1, 3):lower()]
        if not month then
          return nil
        end
        date.month = month
        position = position + #name
      elseif directive == "%" then
        if text:sub(position, position) ~= "%" then
          return nil
        end
        position = position + 1
      else
        -- a directive dates don't need (%a, %p...): skip a word
        local word = text:match("^%S+", position) or ""
        position = position + #word
      end
    else
      if char:match("%s") then
        -- a space in the format matches any number of spaces
        position = (text:match("^%s*()", position))
      elseif text:sub(position, position) ~= char then
        return nil
      else
        position = position + 1
      end
      i = i + 1
    end
  end
  if position <= #text then
    return nil
  end
  if not (date.year and date.month and date.day) then
    return nil
  end
  date.hour, date.min = date.hour or 0, date.min or 0
  if not M.time(date) then
    return nil
  end
  return date
end

-- Whether `format` starts with the year (then "1.2.3" is year.month.day), or the day.
local function year_first(format)
  return format:gsub("^[%(%s]+", ""):match("^%%[yY]") ~= nil
end

local function day_first(format)
  return format:gsub("^[%(%s]+", ""):match("^%%d") ~= nil
end

-- The date `text` (a short date, like PlainTasks's) means at `now` (a date): one of
--   "5"           the 5th of this month, of the next one when today is the 5th or later
--   "1"           the 1st of the next month
--   "2-3"         February 3rd, of next year when February is past
--   "16.1.1"      year 16 (2016), January 1st
--   "31 23:"      the 31st at 23 and the minutes of now
--   "--1"         the 1st of this month
--   "" or "0"     today
-- with a time "hour:minute" after it (missing parts: now's). Nil and an error when it is not
-- a date.
local function short_date(text, now)
  text = text:gsub("^%s+", ""):gsub("%s+$", "")
  local date_part, hour, minute = text, nil, nil
  local before, h, m = text:match("^(.-)%s*(%d*):(%d*)$")
  if before then
    date_part, hour, minute = before, h, m
  end
  local year, month, day = now.year, now.month, nil
  local this_month = date_part:match("^%-%-(%d+)$")
  if this_month then
    day = tonumber(this_month)
  elseif date_part ~= "" then
    local separator = date_part:match("[%-%.]")
    local parts = {}
    if separator then
      for part in (date_part .. separator):gmatch("(.-)%" .. separator) do
        table.insert(parts, part)
      end
    else
      parts = { date_part }
    end
    for _, part in ipairs(parts) do
      if not part:match("^%d+$") then
        return nil, "not a date: " .. text
      end
    end
    if #parts == 3 then
      year, month, day = tonumber(parts[1]), tonumber(parts[2]), tonumber(parts[3])
    elseif #parts == 2 then
      month, day = tonumber(parts[1]), tonumber(parts[2])
      if month < now.month then
        year = year + 1
      end
    elseif #parts == 1 then
      day = tonumber(parts[1])
      if day > 0 and day <= now.day then
        -- that day is past (or today): next month's
        month = month + 1
        if month == 13 then
          year, month = year + 1, 1
        end
      elseif day == 0 then
        day = now.day
      end
    else
      return nil, "not a date: " .. text
    end
  else
    day = now.day
  end
  if year < 100 then
    year = year + 2000
  end
  local date = {
    year = year, month = month, day = day,
    hour = tonumber(hour ~= "" and hour or nil) or now.hour,
    min = tonumber(minute ~= "" and minute or nil) or now.min,
  }
  if not M.time(date) then
    return nil, string.format(
      "not a date: year %d, month %d, day %d, %02d:%02d",
      date.year, date.month, date.day, date.hour, date.min
    )
  end
  return date
end

-- A whole date, written another way than `format`: "2016-01-16 10:30", "16.1.16",
-- "2003-Sep-25", "4-1-16" (day first when `format` starts with the day).
local function full_date(text, format, now)
  local date_part, time_part = text:match("^(%S+)%s+(%d+:%d+)$")
  if not date_part then
    date_part = text
  end
  local separator = date_part:match("[%-%./]")
  if not separator then
    return nil
  end
  local parts = {}
  for part in (date_part .. separator):gmatch("(.-)%" .. separator) do
    table.insert(parts, part)
  end
  if #parts ~= 3 then
    return nil
  end
  local year, month, day
  local named = MONTHS[(parts[2]:sub(1, 3)):lower()]
  if named then
    -- "2003-Sep-25" or "25-Sep-2003"
    month = named
    if #parts[1] == 4 or year_first(format) then
      year, day = tonumber(parts[1]), tonumber(parts[3])
    else
      day, year = tonumber(parts[1]), tonumber(parts[3])
    end
  elseif year_first(format) or #parts[1] == 4 then
    year, month, day = tonumber(parts[1]), tonumber(parts[2]), tonumber(parts[3])
  elseif day_first(format) then
    day, month, year = tonumber(parts[1]), tonumber(parts[2]), tonumber(parts[3])
  else
    month, day, year = tonumber(parts[1]), tonumber(parts[2]), tonumber(parts[3])
  end
  if not (year and month and day) then
    return nil
  end
  if year < 100 then
    year = year + 2000
  end
  local hour, minute = now.hour, now.min
  if time_part then
    local h, m = time_part:match("^(%d+):(%d+)$")
    hour, minute = tonumber(h), tonumber(m)
  end
  local date = { year = year, month = month, day = day, hour = hour, min = minute }
  if not M.time(date) then
    return nil
  end
  return date
end

-- The date `text` (what is in the parentheses of a tag) means, written with `format` or a
-- short date, at `now` (a date): a date, or nil and an error.
function M.parse(text, format, now)
  local date = M.strptime(text, format) or M.strptime("(" .. text .. ")", format)
  if date then
    return date
  end
  local bare = text:gsub("^[%(%s]+", ""):gsub("[%)%s]+$", "")
  -- as PlainTasks: "1" and "5" are short dates, "2-1" too (not this year's February 1st)
  local numbers = select(2, bare:gsub("[%-%.]", "")) + 1
  local short = (numbers < 2 and #bare < 3) or (numbers < 3 and bare:find("[%-%.]") ~= nil)
  if not short then
    date = full_date(bare, format, now)
    if date then
      return date
    end
    -- a year alone ("2033")
    local year = bare:match("^(%d%d%d%d?)$")
    if year then
      date = { year = tonumber(year), month = now.month, day = now.day, hour = now.hour, min = now.min }
      if M.time(date) then
        return date
      end
    end
  end
  return short_date(bare, now)
end

-- `text` like "+", "+2d", "+3w", "+2:", "+:555", "+2 12:" or "++" (from `created` when
-- given): the date that long after `now`, or after `created` for "++".
--   +[+][number][d|w][hours:minutes]   (a missing number is 1, unless a time is given)
function M.increase(text, now, created)
  local base = now
  if text:find("++", 1, true) and created then
    base = created
  end
  local rest = text:gsub("^%s*%+%+?%s*", "")
  local number, unit, hours, minutes
  local before_time, h, m = rest:match("^(.-)(%d*)[:%.](%d*)")
  local count_part = rest
  if before_time then
    hours, minutes = tonumber(h), tonumber(m)
    count_part = before_time
  end
  number, unit = count_part:match("^%s*(%d*)%s*([DdWw]?)")
  number = tonumber(number)
  if number == 0 then
    number = nil
  end
  if hours == 0 then
    hours = nil
  end
  if minutes == 0 then
    minutes = nil
  end
  local weeks = unit == "w" or unit == "W"
  if (not number and not hours and not minutes) or (not number and unit ~= "") then
    number = 1
  end
  local days = (number or 0) * (weeks and 7 or 1)
  if not M.time(base) then
    return nil, "not a date"
  end
  -- in calendar days, hours and minutes (like PlainTasks: across a daylight saving change,
  -- "+1d" is the same time tomorrow)
  local ok, time = pcall(os.time, {
    year = base.year,
    month = base.month,
    day = base.day + days,
    hour = base.hour + (hours or 0),
    min = base.min + (minutes or 0),
    sec = 0,
  })
  if not ok or not time then
    return nil, "not a date"
  end
  return M.date_of(time)
end

-- The date `text` (in the parentheses of @due) means at `now` (a date): relative ones ("+2d")
-- from now, or from `created` ("++"). A date, or nil and an error.
function M.due(text, format, now, created)
  if text:find("+", 1, true) then
    return M.increase(text, now, created)
  end
  return M.parse(text, format, now)
end

-- A length of time, `seconds` long, the way PlainTasks writes it: "2 days, 3:04" ("3:04" for
-- less than a day, "2 days" for whole days), or with the minutes as a part of the hour,
-- "2 days, 3.07", when `decimal`. "" for less than a minute.
function M.format_delta(seconds, decimal)
  seconds = math.floor(seconds)
  local days = seconds // 86400
  local rest = seconds % 86400
  local day_text = ""
  if days == 1 then
    day_text = "1 day, "
  elseif days > 1 then
    day_text = days .. " days, "
  end
  local text
  if decimal then
    text = day_text .. (rest > 0 and string.format("%.2f", rest / 3600) or "")
  else
    local hours, minutes = rest // 3600, (rest % 3600) // 60
    if hours == 0 and minutes == 0 then
      text = day_text
    else
      text = string.format("%s%d:%02d", day_text, hours, minutes)
    end
  end
  return (text:gsub("[%s,]+$", ""))
end

-- The seconds a length written by `format_delta` (or by PlainTasks) stands for: "2 days, 3:04",
-- "3:04:05", "1.50" (hours), "a bit" (0).
function M.parse_delta(text)
  local days = tonumber(text:match("(%d+)%s*days?")) or 0
  local rest = text:gsub("%d+%s*days?,?", "")
  local seconds = days * 86400
  local h, m, s = rest:match("(%d+):(%d+):?(%d*)")
  if h then
    seconds = seconds + tonumber(h) * 3600 + tonumber(m) * 60 + (tonumber(s) or 0)
  else
    local hours = rest:match("(%d+%.%d+)")
    if hours then
      seconds = seconds + math.floor(tonumber(hours) * 3600 + 0.5)
    end
  end
  return seconds
end

-- The time spent on a task started at `started` and paused and resumed at `toggles` (each
-- toggle a pause or a resume, by its place), until `finished`: seconds. Times in seconds.
function M.spent(started, toggles, finished)
  local times = { started }
  for _, toggle in ipairs(toggles) do
    table.insert(times, toggle)
  end
  table.insert(times, finished)
  local total = 0
  for i = 1, #times - 1, 2 do
    total = total + (times[i + 1] - times[i])
  end
  return total
end

-- The date of `time`.
function M.date_of(time)
  local date = os.date("*t", time)
  return { year = date.year, month = date.month, day = date.day, hour = date.hour, min = date.min }
end

return M
