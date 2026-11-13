library ieee;
use ieee.std_logic_1164.all;

entity sample_counter is
  generic (
    WIDTH : positive := 8
  );
  port (
    --# {{clocks|Clock}}
    clk : in std_logic;
    --# {{control|Control}}
    reset_n : in std_logic;
    enable : in std_logic;
    --# {{data|Data}}
    count : out std_logic_vector(WIDTH-1 downto 0);
    bus_io : inout std_logic_vector(3 downto 0)
  );
end entity sample_counter;
