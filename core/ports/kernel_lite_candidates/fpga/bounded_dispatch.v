module nobro_bounded_dispatch #(
    parameter TASKS = 4,
    parameter INDEX_BITS = 2
) (
    input wire clock,
    input wire reset_n,
    input wire [TASKS-1:0] ready,
    output reg valid,
    output reg [INDEX_BITS-1:0] selected
);
    integer offset;
    reg [INDEX_BITS-1:0] cursor;
    always @(posedge clock or negedge reset_n) begin
        if (!reset_n) begin
            cursor <= 0;
            selected <= 0;
            valid <= 0;
        end else begin
            valid <= 0;
            for (offset = TASKS - 1; offset >= 0; offset = offset - 1) begin
                if (ready[(cursor + offset) % TASKS]) begin
                    selected <= (cursor + offset) % TASKS;
                    cursor <= ((cursor + offset) % TASKS) + 1'b1;
                    valid <= 1;
                end
            end
        end
    end
endmodule
