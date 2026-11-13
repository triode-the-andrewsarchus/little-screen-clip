library ieee;
use ieee.std_logic_1164.all;

package preview_components is
  component mixer
    generic (WIDTH : positive := 16);
    port (
      clk : in std_logic;
      left_in, right_in : in std_logic_vector(WIDTH-1 downto 0);
      result : out std_logic_vector(WIDTH downto 0)
    );
  end component mixer;
end package;
