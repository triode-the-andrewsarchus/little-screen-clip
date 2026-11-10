/* Private acceptance source: send only to subscriptions owned by the test PID.
 * Never broadcast, subscribe to hardware, or send to another application's port. */
#include <alsa/asoundlib.h>
#include <stdio.h>
#include <stdlib.h>

static int owns_client(const snd_seq_client_info_t *info, int target) {
    int pid=snd_seq_client_info_get_pid(info);
    if (pid==target) return 1;
    if (pid<=0) return 0;
    char path[64], line[256];
    snprintf(path,sizeof(path),"/proc/%d/status",pid);
    FILE *status=fopen(path,"r");
    if (!status) return 0;
    int group=0;
    while (fgets(line,sizeof(line),status)) {
        if (sscanf(line,"Tgid: %d",&group)==1) break;
    }
    fclose(status);
    return group==target;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    int target = atoi(argv[1]);
    if (target <= 1) return 2;
    snd_seq_t *seq;
    if (snd_seq_open(&seq, "default", SND_SEQ_OPEN_DUPLEX, 0) < 0) return 3;
    snd_seq_set_client_name(seq, "Private multisample acceptance source");
    int port = snd_seq_create_simple_port(seq, "Owned fixture", SND_SEQ_PORT_CAP_READ | SND_SEQ_PORT_CAP_SUBS_READ, SND_SEQ_PORT_TYPE_APPLICATION);
    if (port < 0) return 4;
    snd_seq_addr_t source = {snd_seq_client_id(seq), port};
    snd_seq_query_subscribe_t *query;
    snd_seq_client_info_t *info;
    snd_seq_query_subscribe_alloca(&query);
    snd_seq_client_info_alloca(&info);
    unsigned status, key, value;
    while (scanf("%u %u %u", &status, &key, &value) == 3) {
        int sent = 0;
        snd_seq_query_subscribe_set_root(query, &source);
        snd_seq_query_subscribe_set_type(query, SND_SEQ_QUERY_SUBS_READ);
        for (int index=0; ; ++index) {
            snd_seq_query_subscribe_set_index(query, index);
            if (snd_seq_query_port_subscribers(seq, query) < 0) break;
            const snd_seq_addr_t *dest = snd_seq_query_subscribe_get_addr(query);
            if (snd_seq_get_any_client_info(seq, dest->client, info) < 0 || !owns_client(info,target)) continue;
            if (status == 0) {++sent; continue;}
            snd_seq_event_t event;
            snd_seq_ev_clear(&event);
            if ((status & 0xf0) == 0x90) snd_seq_ev_set_noteon(&event, status & 15, key, value);
            else if ((status & 0xf0) == 0x80) snd_seq_ev_set_noteoff(&event, status & 15, key, value);
            else if ((status & 0xf0) == 0xb0) snd_seq_ev_set_controller(&event, status & 15, key, value);
            else continue;
            snd_seq_ev_set_source(&event, port);
            snd_seq_ev_set_dest(&event, dest->client, dest->port);
            snd_seq_ev_set_direct(&event);
            if (snd_seq_event_output_direct(seq, &event) >= 0) ++sent;
        }
        printf("%d\n", sent);
        fflush(stdout);
    }
    snd_seq_close(seq);
    return 0;
}
